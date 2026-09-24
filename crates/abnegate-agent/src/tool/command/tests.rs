use std::path::Path;
use std::path::PathBuf;

use abnegate_exec::PROXY_URL_VARIABLE;
use serde_json::json;
use tokio::process::Command;

use super::run::ALLOWED_COMMANDS;
use super::run::RunCommandParameters;
use super::shell::RunShellParameters;
use super::shell::total_sleep;
use super::*;
use crate::test_support::CHILD_TEST;
use crate::test_support::captured_logs;
use crate::tool::MAX_TOOL_MESSAGE_CHARACTERS;
use crate::tool::Session;
use crate::tool::Tool;

fn create_test_context() -> ToolContext {
    ToolContext {
        working_directory: std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")),
        environment: crate::tool::EnvironmentPolicy::empty(),
        max_file_size: 1024 * 1024,
        command_timeout: std::time::Duration::from_secs(30),
        unrestricted: false,
        session: Session::Detached,
        application: crate::Application::default(),
    }
}

fn logs(checkout: &Path) -> PathBuf {
    job::log_directory(checkout, &crate::Application::default())
}

/// The environment a child sees, as `run_command` and `run_shell` start it.
///
/// `env` runs any command it is handed, so it is not on `run_command`'s list;
/// the environment `run_command` builds is read through the one function both
/// tools and background jobs start their children with.
async fn environments(context: &ToolContext) -> [String; 2] {
    let command = crate::tool::process::run(
        crate::tool::process::command("env", context),
        std::time::Duration::from_secs(10),
    )
    .await
    .expect("env runs");
    assert!(command.status.success(), "{command:?}");
    let shell = RunShellTool
        .execute(json!({"command": "env"}), context)
        .await
        .unwrap();
    assert!(shell.success, "{shell:?}");
    [command.stdout, shell.output.unwrap()]
}

/// Both shelling tools hand the child only what the context names.
///
/// The context environment *is* the allowlist a caller builds: a server
/// narrows it to a fixed set of names precisely because its own process
/// holds the database URL, the JWT and encryption keys and the provider
/// keys. A child that inherited the parent's environment would print all
/// of it into tool output, so `env_clear` is what makes the caller's
/// allowlist an allowlist.
///
/// The marker is put in this process's environment by running the test again
/// in a child process that starts with it, because setting a variable in a
/// running test binary races every other test reading the environment.
#[tokio::test]
async fn shelling_tools_give_the_child_only_the_context_environment() {
    const NAME: &str =
        "tool::command::tests::shelling_tools_give_the_child_only_the_context_environment";
    const MARKER: &str = "ABNEGATE_COMMAND_ENVIRONMENT_MARKER";
    const VALUE: &str = "must-not-reach-a-child";
    if std::env::var(CHILD_TEST).as_deref() != Ok(NAME) {
        let output = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", NAME, "--nocapture"])
            .env(CHILD_TEST, NAME)
            .env(MARKER, VALUE)
            .output()
            .await
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    assert_eq!(
        std::env::var(MARKER).as_deref(),
        Ok(VALUE),
        "the marker has to be in this process for the test to prove anything"
    );

    let mut context = create_test_context();
    context.environment = crate::tool::EnvironmentPolicy::from_iter([(
        "PATH".to_string(),
        std::env::var("PATH").unwrap_or_default(),
    )]);

    for output in environments(&context).await {
        assert!(output.contains("PATH="), "the tool did not run: {output}");
        assert!(
            !output.contains(MARKER),
            "the process environment reached the child: {output}"
        );
        // `sh` computes these from the working directory it was given.
        const SHELL_OWN: &[&str] = &["PWD", "SHLVL", "_"];
        for (name, _) in output.lines().filter_map(|line| line.split_once('=')) {
            assert!(
                context.environment.contains(name)
                    || name.to_ascii_uppercase().ends_with("_PROXY")
                    || SHELL_OWN.contains(&name),
                "{name} is not on the context environment and must not have survived"
            );
        }
    }
}

fn shell_test_context() -> ToolContext {
    let mut context = create_test_context();
    context.unrestricted = true;
    context.environment.set(
        "PATH".to_string(),
        std::env::var("PATH").unwrap_or_default(),
    );
    context
}

#[tokio::test]
async fn proxy_overrides_command_and_shell_environment() {
    const NAME: &str = "tool::command::tests::proxy_overrides_command_and_shell_environment";
    if std::env::var(CHILD_TEST).as_deref() != Ok(NAME) {
        let output = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", NAME, "--nocapture"])
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env(CHILD_TEST, NAME)
            .env(PROXY_URL_VARIABLE, "http://127.0.0.1:28888")
            .output()
            .await
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let mut context = create_test_context();
    context.environment = crate::tool::EnvironmentPolicy::from_iter([
        ("HTTPS_PROXY".to_string(), "http://wrong:8888".to_string()),
        ("http_proxy".to_string(), "http://wrong:8888".to_string()),
        ("NO_PROXY".to_string(), "*".to_string()),
        ("no_proxy".to_string(), "*".to_string()),
        (PROXY_URL_VARIABLE.to_string(), "".to_string()),
    ]);
    for output in environments(&context).await {
        for key in [
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "http_proxy",
            "https_proxy",
            "all_proxy",
        ] {
            assert!(
                output
                    .lines()
                    .any(|line| line == format!("{key}=http://127.0.0.1:28888")),
                "{output}"
            );
        }
        assert!(
            !output
                .lines()
                .any(|line| line == "NO_PROXY=*" || line == "no_proxy=*")
        );
        assert!(output.contains("NO_PROXY=localhost,127.0.0.1,::1"));
    }
}

#[test]
fn test_run_command_tool_metadata() {
    let tool = RunCommandTool;
    assert_eq!(tool.name(), "run_command");
    assert!(!tool.description().is_empty());

    let schema = tool.parameters_schema();
    assert!(schema.get("properties").is_some());
    assert!(schema.get("required").is_some());
}

#[tokio::test]
async fn test_run_command_echo() {
    let tool = RunCommandTool;
    let context = create_test_context();

    let result = tool
        .execute(
            serde_json::json!({"command": "echo", "args": ["hello", "world"]}),
            &context,
        )
        .await
        .unwrap();

    assert!(result.success);
    assert!(result.output.unwrap().contains("hello world"));
}

#[tokio::test]
async fn test_run_command_pwd() {
    let tool = RunCommandTool;
    let context = create_test_context();

    let result = tool
        .execute(serde_json::json!({"command": "pwd"}), &context)
        .await
        .unwrap();

    assert!(result.success);
    assert!(result.output.unwrap().contains("/"));
}

#[tokio::test]
async fn test_run_command_not_found() {
    let tool = RunCommandTool;
    let context = create_test_context();

    let result = tool
        .execute(
            serde_json::json!({"command": "nonexistent_command_xyz_12345"}),
            &context,
        )
        .await;

    assert!(result.is_err());
}

/// The allowlist was matched against the last path segment while the whole
/// string was executed, so a file the agent had just written into `cwd` and
/// named `cargo` satisfied the list and then ran.
#[cfg(unix)]
#[tokio::test]
async fn an_allowed_name_on_a_path_is_not_an_allowed_program() {
    use std::os::unix::fs::PermissionsExt;

    const MARKER: &str = "arbitrary-execution-marker";
    let directory = tempfile::tempdir().expect("a temporary directory");
    let impostor = directory.path().join("cargo");
    std::fs::write(&impostor, format!("#!/bin/sh\necho {MARKER}\n")).unwrap();
    std::fs::set_permissions(&impostor, std::fs::Permissions::from_mode(0o755)).unwrap();

    let context = ToolContext {
        working_directory: directory.path().canonicalize().unwrap(),
        environment: crate::tool::EnvironmentPolicy::from_iter([(
            "PATH".to_string(),
            std::env::var("PATH").unwrap_or_default(),
        )]),
        ..ToolContext::default()
    };

    for command in [
        "./cargo".to_string(),
        "cargo/../cargo".to_string(),
        impostor.to_string_lossy().into_owned(),
    ] {
        let error = RunCommandTool
            .execute(
                serde_json::json!({"command": command, "args": []}),
                &context,
            )
            .await
            .expect_err("a path must not satisfy the allowlist");
        assert!(
            error.to_string().contains("not in the allowed list"),
            "{command}: {error}"
        );
        assert!(
            !error.to_string().contains(MARKER),
            "{command} ran: {error}"
        );
    }
}

#[tokio::test]
async fn test_run_command_not_in_allowlist() {
    let tool = RunCommandTool;
    let context = create_test_context();

    let result = tool
        .execute(
            serde_json::json!({"command": "rm", "args": ["-rf", "/"]}),
            &context,
        )
        .await;

    assert!(result.is_err());
    let error = result.unwrap_err();
    assert!(error.to_string().contains("not in the allowed list"));
}

#[tokio::test]
async fn test_run_command_dangerous_not_allowed() {
    let tool = RunCommandTool;
    let context = create_test_context();

    let result = tool
        .execute(
            serde_json::json!({"command": "sudo", "args": ["ls"]}),
            &context,
        )
        .await;

    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("not in the allowed list")
    );
}

#[tokio::test]
async fn test_run_command_shell_injection_blocked() {
    let tool = RunCommandTool;
    let context = create_test_context();

    let result = tool
        .execute(
            serde_json::json!({"command": "echo", "args": ["hello; rm -rf /"]}),
            &context,
        )
        .await;

    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("dangerous pattern")
    );
}

#[tokio::test]
async fn test_run_command_pipe_injection_blocked() {
    let tool = RunCommandTool;
    let context = create_test_context();

    let result = tool
        .execute(
            serde_json::json!({"command": "echo", "args": ["hello | cat /etc/passwd"]}),
            &context,
        )
        .await;

    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("dangerous pattern")
    );
}

#[tokio::test]
async fn test_run_command_command_substitution_blocked() {
    let tool = RunCommandTool;
    let context = create_test_context();

    let result = tool
        .execute(
            serde_json::json!({"command": "echo", "args": ["$(whoami)"]}),
            &context,
        )
        .await;

    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("dangerous pattern")
    );
}

#[tokio::test]
async fn test_run_command_allowed_commands() {
    let tool = RunCommandTool;
    let context = create_test_context();

    let allowed = ["cargo", "npm", "git", "go", "ls", "cat"];
    for command in allowed {
        let result = tool
            .execute(
                serde_json::json!({"command": command, "args": ["--version"]}),
                &context,
            )
            .await;
        // A program that is not installed fails to run, which is not a refusal.
        if let Err(error) = &result {
            assert!(
                !error.to_string().contains("not in the allowed list"),
                "Command {command} should be allowed"
            );
        }
    }
}

#[tokio::test]
async fn test_run_command_with_exit_code() {
    let tool = RunCommandTool;
    let context = create_test_context();

    let result = tool
        .execute(serde_json::json!({"command": "false"}), &context)
        .await
        .unwrap();

    assert!(!result.success);
    assert!(result.error.is_some());
}

#[tokio::test]
async fn test_run_command_with_stderr() {
    let tool = RunCommandTool;
    let context = create_test_context();

    let result = tool
        .execute(
            serde_json::json!({"command": "ls", "args": ["/nonexistent_path_xyz_12345"]}),
            &context,
        )
        .await
        .unwrap();

    assert!(!result.success);
    let output = result.error.unwrap();
    assert!(output.contains("stderr"));
}

#[test]
fn test_run_command_tool_definition() {
    let tool = RunCommandTool;
    let definition = tool.to_definition();

    assert_eq!(definition.tool_type, "function");
    assert_eq!(definition.function.name, "run_command");
    assert!(definition.function.description.contains("shell"));
}

#[test]
fn run_command_params_read_the_reason() {
    let parameters: RunCommandParameters = serde_json::from_value(json!({
        "command": "cargo",
        "args": ["test"],
        "reason": "Check the suite still passes before committing."
    }))
    .unwrap();

    assert_eq!(
        parameters.reason.as_deref(),
        Some("Check the suite still passes before committing.")
    );
}

#[tokio::test]
async fn run_command_accepts_a_call_carrying_a_reason() {
    let result = RunCommandTool
        .execute(
            json!({
                "command": "echo",
                "args": ["hello"],
                "reason": "Show the user what the tool returns."
            }),
            &create_test_context(),
        )
        .await
        .unwrap();

    assert!(result.success, "{:?}", result.error);
    assert!(result.output.unwrap().contains("hello"));
}

#[tokio::test]
async fn run_command_without_a_reason_still_runs() {
    let result = RunCommandTool
        .execute(
            json!({"command": "echo", "args": ["hello"]}),
            &create_test_context(),
        )
        .await
        .unwrap();

    assert!(result.success, "{:?}", result.error);
    assert!(result.output.unwrap().contains("hello"));
}

#[test]
fn run_shell_params_read_the_reason() {
    let parameters: RunShellParameters = serde_json::from_value(json!({
        "command": "cargo test 2>&1 | tail -40",
        "reason": "Check the suite still passes before committing."
    }))
    .unwrap();

    assert_eq!(
        parameters.reason.as_deref(),
        Some("Check the suite still passes before committing.")
    );
}

#[tokio::test]
async fn run_shell_accepts_a_call_carrying_a_reason() {
    let result = RunShellTool
        .execute(
            json!({
                "command": "echo hello",
                "reason": "Show the user what the tool returns."
            }),
            &shell_test_context(),
        )
        .await
        .unwrap();

    assert!(result.success, "{:?}", result.error);
    assert!(result.output.unwrap().contains("hello"));
}

#[tokio::test]
async fn run_shell_without_a_reason_still_runs() {
    let result = RunShellTool
        .execute(json!({"command": "echo hello"}), &shell_test_context())
        .await
        .unwrap();

    assert!(result.success, "{:?}", result.error);
    assert!(result.output.unwrap().contains("hello"));
}

/// `sleep` reads a bare number as seconds and a suffix as its unit, and
/// adds its operands up. Reading them the same way is what makes the cap
/// land on the wait that actually happens.
#[test]
fn a_sleep_is_measured_the_way_sleep_itself_reads_its_operands() {
    assert_eq!(total_sleep("sleep 30"), Some(30.0));
    assert_eq!(total_sleep("sleep 0.5"), Some(0.5));
    assert_eq!(total_sleep("sleep 2m"), Some(120.0));
    assert_eq!(total_sleep("sleep 1h"), Some(3_600.0));
    assert_eq!(total_sleep("sleep 1d"), Some(86_400.0));
    assert_eq!(total_sleep("sleep 40 40"), Some(80.0));
    assert_eq!(total_sleep("echo hello"), None);
    assert_eq!(total_sleep("echo sleep 900"), None);
}

/// The wait is what counts, not the shape of the line it hides in: a
/// segment reached by a pipe, a chain, a subshell or a path is still a
/// segment whose command is `sleep`, and every one of them adds to the
/// wait the caller is about to sit through.
#[test]
fn a_sleep_is_found_wherever_a_command_can_start() {
    assert_eq!(total_sleep("cargo build && sleep 300"), Some(300.0));
    assert_eq!(total_sleep("sleep 300; cargo test"), Some(300.0));
    assert_eq!(total_sleep("sleep 10 || sleep 300"), Some(310.0));
    assert_eq!(total_sleep("(sleep 300)"), Some(300.0));
    assert_eq!(total_sleep("{ sleep 300; }"), Some(300.0));
    assert_eq!(total_sleep("/bin/sleep 300"), Some(300.0));
    assert_eq!(total_sleep("DELAY=1 sleep 300"), Some(300.0));
    assert_eq!(total_sleep("sleep 300 | cat"), Some(300.0));
    assert_eq!(total_sleep("cargo build & sleep 300"), Some(300.0));
}

/// The cap is on how long the call blocks, and a line blocks for the sum
/// of its sleeps. Comparing only the longest one let a call wait for as
/// many multiples of the cap as it cared to write out.
#[test]
fn sleeps_in_sequence_add_up_to_the_wait_the_cap_is_measured_against() {
    assert_eq!(total_sleep("sleep 40; sleep 40"), Some(80.0));
    assert_eq!(total_sleep("sleep 30 && sleep 30 && sleep 30"), Some(90.0));
    assert_eq!(total_sleep("sleep 20 | cat; sleep 50"), Some(70.0));
}

/// An operand this cannot read is not an excuse to reject the call. The
/// per-call timeout is still behind it, and refusing what might be a
/// one-second wait would cost more than letting it through.
#[test]
fn an_unreadable_operand_ends_the_sum_rather_than_the_call() {
    assert_eq!(total_sleep("sleep $DELAY"), Some(0.0));
    assert_eq!(total_sleep("sleep 30 $DELAY 300"), Some(30.0));
}

/// A negative operand is not a wait to be credited against a real one.
/// `sleep -100; sleep 120` summed to twenty and was let through, and then
/// `sh` failed the first segment and blocked for the full two minutes on
/// the second.
#[test]
fn a_negative_operand_buys_no_credit_against_a_real_wait() {
    assert_eq!(total_sleep("sleep -100; sleep 120"), Some(120.0));
    assert_eq!(total_sleep("sleep -100"), Some(0.0));
    assert_eq!(total_sleep("sleep -5m"), Some(0.0));
    assert_eq!(total_sleep("sleep -100 120"), Some(120.0));
}

/// The task loop announces a stall after the same interval, so a call that
/// blocks past it would look wedged rather than waiting.
///
/// The one-second limit is what makes the refusal visible: without the cap
/// the call reaches the shell and fails on the limit instead, so the two
/// outcomes cannot be confused for one another.
#[tokio::test]
async fn a_shell_call_may_not_block_on_sleep_past_the_cap() {
    let error = RunShellTool
        .execute(
            json!({
                "command": "sleep 300",
                "timeout_secs": 1,
                "reason": "Wait for the deploy."
            }),
            &shell_test_context(),
        )
        .await
        .expect_err("a sleep past the cap is refused");

    let message = error.to_string();
    assert!(message.contains("300"), "{message}");
    assert!(
        message.contains(&MAX_SLEEP_SECONDS.to_string()),
        "the refusal names the cap it enforces: {message}"
    );
}

/// The cap is on waiting, not on `sleep`: a short one is how a command
/// legitimately lets something settle, and it still runs.
#[tokio::test]
async fn a_shell_call_may_still_sleep_inside_the_cap() {
    let result = RunShellTool
        .execute(
            json!({"command": "sleep 0.01 && echo settled"}),
            &shell_test_context(),
        )
        .await
        .unwrap();

    assert!(result.success, "{:?}", result.error);
    assert!(result.output.unwrap().contains("settled"));
}

/// The model is told the rule in the schema, so the first it hears of the
/// cap is not a call that failed on it.
#[test]
fn the_shell_schema_states_the_sleep_cap() {
    let schema = RunShellTool.parameters_schema();
    let described = schema["properties"]["command"]["description"]
        .as_str()
        .unwrap()
        .to_string();

    assert!(
        described.contains(&MAX_SLEEP_SECONDS.to_string()),
        "{described}"
    );
    assert!(described.contains("sleep"), "{described}");
}

/// A reason is the model's own prose and can carry whatever it just read
/// out of a file or a page, so the run log records that one arrived and
/// never what it said.
#[tokio::test]
async fn the_shell_tools_log_that_a_reason_arrived_without_repeating_it() {
    const LIFTED: &str = "AWS_SECRET_ACCESS_KEY read out of the .env I just opened";

    let (_, command_log) = captured_logs(RunCommandTool.execute(
        json!({"command": "echo", "args": ["hello"], "reason": LIFTED}),
        &create_test_context(),
    ))
    .await;
    let (_, shell_log) = captured_logs(RunShellTool.execute(
        json!({"command": "echo hello", "reason": LIFTED}),
        &shell_test_context(),
    ))
    .await;

    for (tool, logged) in [("run_command", command_log), ("run_shell", shell_log)] {
        assert!(logged.contains("Running tool"), "{logged}");
        assert!(logged.contains(tool), "{logged}");
        assert!(logged.contains("reason_given=true"), "{logged}");
        assert!(
            !logged.contains(LIFTED),
            "{tool} wrote the model's reason to the log: {logged}"
        );
    }
}

#[tokio::test]
async fn a_missing_or_blank_reason_logs_as_none_given() {
    let (_, missing) = captured_logs(RunCommandTool.execute(
        json!({"command": "echo", "args": ["hello"]}),
        &create_test_context(),
    ))
    .await;
    let (_, blank) = captured_logs(RunShellTool.execute(
        json!({"command": "echo hello", "reason": "   "}),
        &shell_test_context(),
    ))
    .await;

    assert!(missing.contains("reason_given=false"), "{missing}");
    assert!(blank.contains("reason_given=false"), "{blank}");
}

fn huge_output_context(body: &str) -> (tempfile::TempDir, ToolContext) {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("huge.txt"), body).unwrap();
    let mut context = shell_test_context();
    context.working_directory = directory.path().to_path_buf();
    (directory, context)
}

#[tokio::test]
async fn run_shell_output_keeps_its_tail_through_to_message() {
    let body = format!(
        "HEAD_MARKER{}TAIL_MARKER",
        "x".repeat(MAX_SHELL_OUTPUT_CHARACTERS * 4)
    );
    let (_dir, context) = huge_output_context(&body);

    let result = RunShellTool
        .execute(json!({"command": "cat huge.txt"}), &context)
        .await
        .unwrap();

    let message = result.to_message();
    assert!(message.contains("HEAD_MARKER"), "{message}");
    assert!(
        message.contains("TAIL_MARKER"),
        "the transcript cut threw away the end of the output: {message}"
    );
    assert!(
        message.chars().count() <= MAX_TOOL_MESSAGE_CHARACTERS,
        "{}",
        message.chars().count()
    );
}

#[test]
fn max_output_chars_clamps_into_range() {
    assert_eq!(clamp_output_characters(None), MAX_SHELL_OUTPUT_CHARACTERS);
    assert_eq!(clamp_output_characters(Some(2_000)), 2_000);
    assert_eq!(
        clamp_output_characters(Some(MAX_SHELL_OUTPUT_CHARACTERS as u64 * 100)),
        MAX_SHELL_OUTPUT_CHARACTERS,
        "the knob must never raise the ceiling"
    );
    assert_eq!(
        clamp_output_characters(Some(u64::MAX)),
        MAX_SHELL_OUTPUT_CHARACTERS
    );
    assert_eq!(
        clamp_output_characters(Some(0)),
        MIN_SHELL_OUTPUT_CHARACTERS
    );
}

#[test]
fn params_read_an_optional_max_output_chars() {
    let command: RunCommandParameters =
        serde_json::from_value(json!({"command": "cargo", "max_output_chars": 2_000})).unwrap();
    assert_eq!(command.max_output_characters, Some(2_000));

    let shell: RunShellParameters =
        serde_json::from_value(json!({"command": "cargo test", "max_output_chars": 2_000}))
            .unwrap();
    assert_eq!(shell.max_output_characters, Some(2_000));

    let without: RunShellParameters =
        serde_json::from_value(json!({"command": "cargo test"})).unwrap();
    assert_eq!(without.max_output_characters, None);
}

/// The knob clamps every number it is given, but only after serde has
/// parsed one into a `u64`. A bare `"type": "integer"` advertises negatives
/// the parser then refuses, which fails the whole call rather than clamping
/// it — so the schema has to rule out what the parser cannot take. Zero is
/// legal and clamps up, which is why the floor is here and not 500.
#[test]
fn the_schema_refuses_the_negative_max_output_chars_the_parser_cannot_read() {
    for schema in [
        RunCommandTool.parameters_schema(),
        RunShellTool.parameters_schema(),
    ] {
        assert_eq!(
            schema["properties"][MAX_OUTPUT_PARAMETER]["minimum"],
            json!(0)
        );
    }

    assert!(
        serde_json::from_value::<RunCommandParameters>(
            json!({"command": "cargo", "max_output_chars": -1})
        )
        .is_err(),
        "a negative would have to clamp rather than fail, so the schema must exclude it"
    );
    assert!(
        serde_json::from_value::<RunShellParameters>(
            json!({"command": "cargo test", "max_output_chars": -1})
        )
        .is_err(),
        "a negative would have to clamp rather than fail, so the schema must exclude it"
    );
    assert_eq!(
        clamp_output_characters(Some(0)),
        MIN_SHELL_OUTPUT_CHARACTERS
    );
}

#[test]
fn shell_schemas_offer_max_output_chars_without_requiring_it() {
    for schema in [
        RunCommandTool.parameters_schema(),
        RunShellTool.parameters_schema(),
    ] {
        assert_eq!(
            schema["properties"][MAX_OUTPUT_PARAMETER]["type"],
            "integer"
        );
        assert!(
            !schema["required"]
                .as_array()
                .expect("required array")
                .iter()
                .any(|name| name.as_str() == Some(MAX_OUTPUT_PARAMETER))
        );
    }
}

#[tokio::test]
async fn run_shell_spends_only_the_requested_max_output_chars() {
    const REQUESTED: usize = 2_000;
    let body = format!(
        "HEAD_MARKER{}TAIL_MARKER",
        "x".repeat(MAX_SHELL_OUTPUT_CHARACTERS * 4)
    );
    let (_dir, context) = huge_output_context(&body);

    let message = RunShellTool
        .execute(
            json!({"command": "cat huge.txt", "max_output_chars": REQUESTED}),
            &context,
        )
        .await
        .unwrap()
        .to_message();

    assert!(message.contains("HEAD_MARKER"), "{message}");
    assert!(message.contains("TAIL_MARKER"), "{message}");
    let chars = message.chars().count();
    assert!(chars <= REQUESTED, "{chars}");
    assert!(chars > REQUESTED - 100, "{chars}");
}

#[tokio::test]
async fn run_shell_cannot_raise_the_cap_above_the_constant() {
    let body = format!(
        "HEAD_MARKER{}TAIL_MARKER",
        "x".repeat(MAX_SHELL_OUTPUT_CHARACTERS * 4)
    );
    let (_dir, context) = huge_output_context(&body);

    let message = RunShellTool
        .execute(
            json!({"command": "cat huge.txt", "max_output_chars": 1_000_000}),
            &context,
        )
        .await
        .unwrap()
        .to_message();

    let chars = message.chars().count();
    assert!(chars <= MAX_SHELL_OUTPUT_CHARACTERS, "{chars}");
    assert!(chars > MAX_SHELL_OUTPUT_CHARACTERS - 100, "{chars}");
    assert!(message.contains("TAIL_MARKER"), "{message}");
}

#[tokio::test]
async fn run_command_honours_a_smaller_max_output_chars() {
    const REQUESTED: usize = 2_000;
    let directory = tempfile::tempdir().unwrap();
    let body = format!("HEAD_MARKER{}TAIL_MARKER", "x".repeat(40_000));
    std::fs::write(directory.path().join("huge.txt"), &body).unwrap();

    let mut context = create_test_context();
    context.working_directory = directory.path().to_path_buf();

    let message = RunCommandTool
        .execute(
            json!({"command": "cat", "args": ["huge.txt"], "max_output_chars": REQUESTED}),
            &context,
        )
        .await
        .unwrap()
        .to_message();

    assert!(message.contains("HEAD_MARKER"), "{message}");
    assert!(message.contains("TAIL_MARKER"), "{message}");
    assert!(message.chars().count() <= REQUESTED, "{message}");
}

/// The failed branch trims the body to the cap and `to_message` then
/// prepends `Error: `, so what the model reads ran over the cap the caller
/// asked for. The framing is paid for out of the budget, the same rule the
/// success path and the transcript cap already follow.
#[tokio::test]
async fn a_failed_command_pays_for_the_error_prefix_out_of_the_requested_cap() {
    const REQUESTED: usize = 2_000;
    let directory = tempfile::tempdir().unwrap();
    let body = format!("HEAD_MARKER{}TAIL_MARKER", "x".repeat(40_000));
    std::fs::write(directory.path().join("huge.txt"), &body).unwrap();

    let mut context = create_test_context();
    context.working_directory = directory.path().to_path_buf();

    let result = RunCommandTool
        .execute(
            json!({
                "command": "cat",
                "args": ["huge.txt", "missing.txt"],
                "max_output_chars": REQUESTED
            }),
            &context,
        )
        .await
        .unwrap();

    assert!(!result.success, "cat of a missing file exits non-zero");
    let message = result.to_message();
    assert!(
        message.starts_with("Error: Command exited with"),
        "{message}"
    );
    assert!(message.contains("HEAD_MARKER"), "{message}");
    assert!(message.contains("TAIL_MARKER"), "{message}");
    assert!(message.contains("characters trimmed"), "{message}");

    let chars = message.chars().count();
    assert!(
        chars <= REQUESTED,
        "the model was handed {chars} characters against a cap of {REQUESTED}"
    );
    assert!(chars > REQUESTED - 100, "{chars}");
}

#[tokio::test]
async fn run_command_trims_huge_stdout() {
    let directory = tempfile::tempdir().unwrap();
    let body = format!("HEAD_MARKER{}TAIL_MARKER", "x".repeat(40_000));
    std::fs::write(directory.path().join("huge.txt"), &body).unwrap();

    let mut context = create_test_context();
    context.working_directory = directory.path().to_path_buf();

    let result = RunCommandTool
        .execute(
            serde_json::json!({"command": "cat", "args": ["huge.txt"]}),
            &context,
        )
        .await
        .unwrap();

    assert!(result.success);
    let output = result.output.unwrap();
    assert!(output.contains("HEAD_MARKER"), "{output}");
    assert!(output.contains("TAIL_MARKER"), "{output}");
    assert!(output.contains("characters trimmed"), "{output}");
    assert!(output.chars().count() <= MAX_SHELL_OUTPUT_CHARACTERS);
    assert!(output.chars().count() < body.chars().count());
}

#[tokio::test]
async fn run_command_trims_huge_error_payload() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(
        directory.path().join("left.txt"),
        format!("HEAD_LEFT{}TAIL_LEFT", "x".repeat(40_000)),
    )
    .unwrap();
    std::fs::write(
        directory.path().join("right.txt"),
        format!("HEAD_RIGHT{}TAIL_RIGHT", "y".repeat(40_000)),
    )
    .unwrap();

    let mut context = create_test_context();
    context.working_directory = directory.path().to_path_buf();

    let result = RunCommandTool
        .execute(
            serde_json::json!({"command": "diff", "args": ["left.txt", "right.txt"]}),
            &context,
        )
        .await
        .unwrap();

    assert!(!result.success);
    let error = result.error.unwrap();
    assert!(error.contains("characters trimmed"), "{error}");
    assert!(error.chars().count() <= MAX_SHELL_OUTPUT_CHARACTERS);
    assert!(
        error.contains("HEAD_LEFT") || error.contains("Command exited"),
        "{error}"
    );
    assert!(
        error.contains("TAIL_RIGHT") || error.contains("TAIL_LEFT"),
        "{error}"
    );
}

fn background_context() -> (tempfile::TempDir, ToolContext, Session) {
    let directory = tempfile::tempdir().expect("a temporary working directory");
    let session = Session::Chat(uuid::Uuid::new_v4());
    let mut context = shell_test_context();
    context.working_directory = directory.path().to_path_buf();
    context.session = session;
    (directory, context, session)
}

/// The receipt is the only channel a tool has to the chat layer, so its
/// shape is pinned here rather than left to whatever the spawn returned.
#[tokio::test]
async fn a_backgrounded_shell_call_returns_the_spawn_receipt() {
    let (directory, context, session) = background_context();

    let result = RunShellTool
        .execute(
            json!({
                "command": "printf 'detached\\n'",
                "background": true,
                "reason": "Start the long one."
            }),
            &context,
        )
        .await
        .expect("the job starts");

    assert!(result.success, "{result:?}");
    let output = result.output.expect("a receipt");
    let id = job::parse_started(&output).expect("the receipt names the job");
    let pid = output
        .split("(pid ")
        .nth(1)
        .and_then(|rest| rest.split(')').next())
        .expect("the receipt names the process");
    assert_eq!(
        output,
        format!(
            "Started {id} (pid {pid}). Log: {}\nWait for it with wait_for, or read it with \
             tail_job.",
            job::log_path(directory.path(), &crate::Application::default(), &id).display()
        )
    );
    assert!(
        job::log_path(directory.path(), &crate::Application::default(), &id).exists(),
        "the log is where the receipt says it is"
    );

    Jobs::kill_session(session).await;
}

/// The refusal a detached call gets has to be actionable by that call.
/// "Background it" is what it already did, and the live consequence of
/// saying so was the model reissuing the identical call until the turn was
/// abandoned with no answer at all.
#[tokio::test]
async fn the_sleep_cap_does_not_tell_a_backgrounded_call_to_background_itself() {
    let (directory, context, session) = background_context();

    let error = RunShellTool
        .execute(
            json!({
                "command": format!("sleep {}", MAX_SLEEP_SECONDS + 60),
                "background": true,
                "reason": "Wait for the deploy."
            }),
            &context,
        )
        .await
        .expect_err("the cap holds whichever way the command runs");

    let message = error.to_string();
    assert!(
        !message.contains(&format!("{BACKGROUND_PARAMETER}: true")),
        "the call had already done that: {message}"
    );
    assert!(
        message.contains(WAIT_FOR) && message.contains(&MAX_SLEEP_SECONDS.to_string()),
        "the refusal still says what the cap is and what to do instead: {message}"
    );
    assert!(
        !logs(directory.path()).exists(),
        "the refusal still comes before anything is spawned"
    );

    Jobs::kill_session(session).await;
}

#[tokio::test]
async fn a_backgrounded_command_call_returns_the_spawn_receipt() {
    let (directory, context, session) = background_context();

    let result = RunCommandTool
        .execute(
            json!({
                "command": "echo",
                "args": ["detached"],
                "background": true,
                "reason": "Start the long one."
            }),
            &context,
        )
        .await
        .expect("the job starts");

    assert!(result.success, "{result:?}");
    let output = result.output.expect("a receipt");
    let id = job::parse_started(&output).expect("the receipt names the job");
    assert!(
        output.starts_with(&format!("Started {id} (pid ")),
        "{output}"
    );
    assert!(
        output.ends_with("Wait for it with wait_for, or read it with tail_job."),
        "{output}"
    );
    assert!(
        job::log_path(directory.path(), &crate::Application::default(), &id).exists(),
        "{output}"
    );

    Jobs::kill_session(session).await;
}

/// A call that says nothing about backgrounding still blocks, so nothing
/// about an existing call changes under it.
#[tokio::test]
async fn a_call_that_asks_for_no_background_still_runs_in_the_foreground() {
    let (directory, context, session) = background_context();

    let shell = RunShellTool
        .execute(json!({"command": "printf 'inline\\n'"}), &context)
        .await
        .expect("the command runs");
    let command = RunCommandTool
        .execute(json!({"command": "echo", "args": ["inline"]}), &context)
        .await
        .expect("the command runs");

    for result in [shell, command] {
        assert!(result.success, "{result:?}");
        let output = result.output.expect("output");
        assert!(output.contains("inline"), "{output}");
        assert!(job::parse_started(&output).is_none(), "{output}");
    }
    assert!(
        !logs(directory.path()).exists(),
        "a foreground call writes no job log"
    );

    Jobs::kill_session(session).await;
}

/// Backgrounding moves who waits, not what a command is allowed to do, so
/// the sleep cap is measured before the job is ever spawned.
#[tokio::test]
async fn a_backgrounded_shell_call_may_not_block_on_sleep_past_the_cap() {
    let (directory, context, session) = background_context();

    let error = RunShellTool
        .execute(
            json!({
                "command": "sleep 300",
                "background": true,
                "reason": "Wait for the deploy."
            }),
            &context,
        )
        .await
        .expect_err("a sleep past the cap is refused whichever way it runs");

    let message = error.to_string();
    assert!(message.contains("300"), "{message}");
    assert!(
        message.contains(&MAX_SLEEP_SECONDS.to_string()),
        "{message}"
    );
    assert!(
        !logs(directory.path()).exists(),
        "the refusal came before anything was spawned"
    );

    Jobs::kill_session(session).await;
}

#[tokio::test]
async fn a_backgrounded_command_still_answers_to_the_allow_list() {
    let (directory, context, session) = background_context();

    let error = RunCommandTool
        .execute(
            json!({
                "command": "rm",
                "args": ["-rf", "."],
                "background": true,
                "reason": "Clean up."
            }),
            &context,
        )
        .await
        .expect_err("an unlisted program is refused whichever way it runs");

    assert!(
        error.to_string().contains("not in the allowed list"),
        "{error}"
    );
    assert!(!logs(directory.path()).exists());

    Jobs::kill_session(session).await;
}

/// An argument reaches the child whole, so one the foreground inspects for
/// metacharacters cannot be smuggled past it by detaching.
#[tokio::test]
async fn a_backgrounded_command_still_refuses_a_metacharacter_argument() {
    let (directory, context, session) = background_context();

    let error = RunCommandTool
        .execute(
            json!({
                "command": "echo",
                "args": ["safe; rm -rf ."],
                "background": true,
                "reason": "Print something."
            }),
            &context,
        )
        .await
        .expect_err("a metacharacter is refused whichever way it runs");

    assert!(error.to_string().contains("dangerous pattern"), "{error}");
    assert!(!logs(directory.path()).exists());

    Jobs::kill_session(session).await;
}

/// A checkout and a directory beside it, which is what a `..` in the
/// model's `cwd` reaches for.
fn neighbours() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let root = tempfile::tempdir().expect("a temporary root");
    let checkout = root.path().join("checkout");
    let elsewhere = root.path().join("elsewhere");
    std::fs::create_dir(&checkout).expect("the session's own tree is created");
    std::fs::create_dir(&elsewhere).expect("a directory beside it is created");
    (root, checkout, elsewhere)
}

/// The directory a call names is the child's, and nothing else's.
///
/// A chat's context is unrestricted, so `..` is the chat's to name — and
/// the receipt still has to promise a log under the tree the session gave
/// the tool, because that tree is the one `tail_job` reads and the one a
/// teardown clears.
#[tokio::test]
async fn a_directory_outside_the_tree_moves_the_child_and_not_the_log() {
    let (_root, checkout, elsewhere) = neighbours();
    let session = Session::Chat(uuid::Uuid::new_v4());
    let mut context = shell_test_context();
    context.working_directory = checkout.clone();
    context.session = session;
    let call = json!({
        "command": "pwd",
        "cwd": "../elsewhere",
        "reason": "Look next door."
    });

    let inline = RunShellTool
        .execute(call.clone(), &context)
        .await
        .expect("the command runs");
    let ran_in = inline.output.expect("output");
    assert_eq!(
        std::fs::canonicalize(ran_in.lines().nth(1).unwrap_or_default()).ok(),
        std::fs::canonicalize(&elsewhere).ok(),
        "the foreground runs where the call said: {ran_in}"
    );

    let mut detached = call;
    detached["background"] = json!(true);
    let started = RunShellTool
        .execute(detached, &context)
        .await
        .expect("the job starts");
    let receipt = started.output.expect("a receipt");
    let id = job::parse_started(&receipt).expect("the receipt names the job");

    assert!(
        receipt.contains(
            &job::log_path(&checkout, &crate::Application::default(), &id)
                .display()
                .to_string()
        ),
        "the receipt names a log outside the session's tree: {receipt}"
    );
    assert!(
        job::log_path(&checkout, &crate::Application::default(), &id).exists(),
        "the log is not where the receipt says it is: {receipt}"
    );
    assert!(
        !logs(&elsewhere).exists(),
        "the log tree followed the directory the call named"
    );

    Jobs::kill_session(session).await;
}

/// Both tools confine the directory they are given the way they confine
/// every other path a model supplies, and both modes answer the same:
/// backgrounding is not a way to run where the foreground would not.
#[tokio::test]
async fn a_directory_outside_the_tree_is_refused_the_same_way_in_both_modes() {
    let (_root, checkout, elsewhere) = neighbours();
    let mut context = create_test_context();
    context.working_directory = checkout.clone();
    context.session = Session::Task(uuid::Uuid::new_v4());
    context.environment.set(
        "PATH".to_string(),
        std::env::var("PATH").unwrap_or_default(),
    );

    for named in [
        elsewhere.to_string_lossy().into_owned(),
        "../elsewhere".to_string(),
    ] {
        for background in [false, true] {
            let call = json!({
                "command": "ls",
                "cwd": named,
                "background": background,
                "reason": "Look next door."
            });
            for tool in ["run_command", "run_shell"] {
                let refused = match tool {
                    "run_command" => RunCommandTool.execute(call.clone(), &context).await,
                    _ => RunShellTool.execute(call.clone(), &context).await,
                }
                .expect_err("a directory outside the tree is refused");
                assert!(
                    refused.to_string().contains("escapes working directory"),
                    "{tool} answered {named} with background={background} in other words: \
                     {refused}"
                );
            }
        }
    }

    assert!(
        !logs(&elsewhere).exists() && !logs(&checkout).exists(),
        "a refusal came after something was already spawned"
    );
}

#[tokio::test]
async fn a_detached_context_cannot_start_a_background_job() {
    let directory = tempfile::tempdir().expect("a temporary working directory");
    let mut context = shell_test_context();
    context.working_directory = directory.path().to_path_buf();

    let shell = RunShellTool
        .execute(json!({"command": "true", "background": true}), &context)
        .await
        .expect_err("a detached context has no session to key a job to");
    let command = RunCommandTool
        .execute(json!({"command": "true", "background": true}), &context)
        .await
        .expect_err("a detached context has no session to key a job to");

    for error in [shell, command] {
        assert!(error.to_string().contains(job::UNAVAILABLE), "{error}");
    }
    assert!(!logs(directory.path()).exists());
}

#[test]
fn both_shell_schemas_offer_background_and_describe_it_the_same_way() {
    let shell = RunShellTool.parameters_schema();
    let command = RunCommandTool.parameters_schema();

    let described = shell["properties"][BACKGROUND_PARAMETER]["description"]
        .as_str()
        .expect("run_shell describes background");
    assert_eq!(
        command["properties"][BACKGROUND_PARAMETER]["description"]
            .as_str()
            .expect("run_command describes background"),
        described
    );
    assert_eq!(
        shell["properties"][BACKGROUND_PARAMETER]["type"],
        json!("boolean")
    );
    assert_eq!(
        described,
        "Detach and return immediately with a job id and log path. Use for anything \
         long-running; wait for it with wait_for instead of blocking. A background job ends \
         with the turn that started it, or with the run. Default false."
    );
    for schema in [&shell, &command] {
        assert!(
            !schema["required"]
                .as_array()
                .unwrap()
                .contains(&json!(BACKGROUND_PARAMETER)),
            "background is optional"
        );
    }
}

#[test]
fn params_default_to_the_foreground() {
    let shell: RunShellParameters = serde_json::from_value(json!({"command": "true"})).unwrap();
    let command: RunCommandParameters = serde_json::from_value(json!({"command": "true"})).unwrap();

    assert!(!shell.background);
    assert!(!command.background);
}

/// Both places the model is told a wait is too long now name the way to
/// take it, rather than sending it round the loop to look again.
#[test]
fn a_long_wait_is_pointed_at_a_background_job_rather_than_another_call() {
    let described = RunShellTool.parameters_schema()["properties"]["command"]["description"]
        .as_str()
        .unwrap()
        .to_string();

    assert!(described.contains(BACKGROUND_PARAMETER), "{described}");
    assert!(described.contains(WAIT_FOR), "{described}");
    assert!(!described.contains("later call"), "{described}");
}

#[tokio::test]
async fn the_sleep_refusal_points_at_a_background_job_rather_than_another_call() {
    let error = RunShellTool
        .execute(
            json!({"command": "sleep 5m", "reason": "Wait for the deploy."}),
            &shell_test_context(),
        )
        .await
        .expect_err("a sleep past the cap is refused");

    let message = error.to_string();
    assert!(message.contains(BACKGROUND_PARAMETER), "{message}");
    assert!(message.contains(WAIT_FOR), "{message}");
    assert!(!message.contains("later call"), "{message}");
}

/// `sleep` inherits the output pipe `sh` was given, so `output()` waited out
/// the whole sleep, and when the call gave up it killed `sh` alone. The call
/// now ends when `sh` does and takes the sleep with it.
#[tokio::test]
async fn a_shell_call_that_leaves_a_child_behind_returns_and_takes_the_child_with_it() {
    let context = shell_test_context();
    let started = std::time::Instant::now();

    let result = RunShellTool
        .execute(
            json!({"command": "sleep 30 & echo $!", "timeout_secs": 3}),
            &context,
        )
        .await
        .expect("the call returns");

    assert!(
        started.elapsed() < std::time::Duration::from_secs(3),
        "the call waited on the background child: {:?}",
        started.elapsed()
    );
    let output = result.output.expect("the child's pid");
    let pid: i32 = output
        .lines()
        .find_map(|line| line.trim().parse().ok())
        .unwrap_or_else(|| panic!("no pid in {output}"));
    let mut gone = false;
    for _ in 0..300 {
        if nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_err() {
            gone = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(gone, "sleep {pid} outlived the call");
}

/// Each of these runs whatever code its arguments hand it, so leaving one on
/// the list made the list a formality: `env sh -c ...`, `python -c ...`,
/// `docker run ...`.
#[tokio::test]
async fn a_program_that_runs_any_code_it_is_given_is_not_on_the_list() {
    const EXECUTORS: &[&str] = &[
        "env", "sh", "bash", "zsh", "python", "python3", "node", "ruby", "perl", "deno", "bun",
        "docker",
    ];
    for program in EXECUTORS {
        assert!(!ALLOWED_COMMANDS.contains(program), "{program} is allowed");
        let error = RunCommandTool
            .execute(
                json!({"command": program, "args": ["-c", "true"]}),
                &create_test_context(),
            )
            .await
            .expect_err("the program is refused");
        assert!(
            error.to_string().contains("not in the allowed list"),
            "{program}: {error}"
        );
    }
}

/// `run_command` took any `timeout_secs` at face value, so a call could ask
/// to run for a year; it is now held to the shell's range, both ends.
#[test]
fn a_call_limit_is_held_between_a_second_and_the_shell_maximum() {
    let default = std::time::Duration::from_secs(300);
    assert_eq!(call_limit(None, default), default);
    assert_eq!(
        call_limit(Some(42), default),
        std::time::Duration::from_secs(42)
    );
    assert_eq!(
        call_limit(Some(0), default),
        std::time::Duration::from_secs(1)
    );
    assert_eq!(
        call_limit(Some(31_536_000), default),
        std::time::Duration::from_secs(MAX_SHELL_TIMEOUT_SECONDS)
    );
    assert_eq!(
        call_limit(None, std::time::Duration::from_secs(86_400)),
        std::time::Duration::from_secs(MAX_SHELL_TIMEOUT_SECONDS)
    );
    assert!(
        RunCommandTool.timeout(&create_test_context())
            > std::time::Duration::from_secs(MAX_SHELL_TIMEOUT_SECONDS),
        "the outer bound must not pre-empt the longest call"
    );
}

/// The model-facing keys are a wire format: the Rust fields behind them are
/// spelled out in full, and the schema and the parser must keep agreeing on
/// the short keys models were prompted with.
#[test]
fn the_shell_schemas_keep_their_wire_keys() {
    for (tool, keys) in [
        (
            &RunCommandTool as &dyn Tool,
            &["command", "args", "cwd", "timeout_secs", "max_output_chars"][..],
        ),
        (
            &RunShellTool,
            &["command", "cwd", "timeout_secs", "max_output_chars"][..],
        ),
    ] {
        let schema = tool.parameters_schema();
        for key in keys {
            assert!(
                schema["properties"].get(*key).is_some(),
                "{} lost {key}",
                tool.name()
            );
        }
    }
    let parsed: RunCommandParameters = serde_json::from_value(json!({
        "command": "git",
        "args": ["status"],
        "cwd": "sub",
        "timeout_secs": 5,
        "max_output_chars": 900
    }))
    .unwrap();
    assert_eq!(parsed.arguments, ["status"]);
    assert_eq!(parsed.working_directory.as_deref(), Some("sub"));
    assert_eq!(parsed.timeout_seconds, Some(5));
    assert_eq!(parsed.max_output_characters, Some(900));
}
