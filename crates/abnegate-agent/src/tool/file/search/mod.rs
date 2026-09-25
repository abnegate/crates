mod parameters;

use std::ffi::OsString;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use parameters::SearchCodeParameters;
use serde_json::Value;
use serde_json::json;
use tokio::io::AsyncBufReadExt;
use tokio::io::BufReader;
use tokio::process::Command;
use tokio::time::Instant;
use tokio::time::timeout_at;

use super::confine;
use super::read_text;
use super::resolve;
use super::walk::Visit;
use super::walk::WALK_TIME_LIMIT;
use super::walk::Walk;
use crate::tool::Tool;
use crate::tool::ToolContext;
use crate::tool::ToolError;
use crate::tool::ToolResult;
use crate::tool::process;

pub(super) const MAXIMUM_SEARCH_RESULTS: usize = 100;

const RIPGREP: &str = "rg";

/// What ripgrep exits with when it searched everything and matched nothing.
const RIPGREP_NO_MATCHES: i32 = 1;

/// Widest matching line ripgrep prints whole; a wider one is cut to a
/// preview, so one minified file cannot fill a result.
const RIPGREP_MAXIMUM_COLUMNS: &str = "400";

/// Build output and dependency trees a search walks past.
const SKIPPED_DIRECTORIES: &[&str] = &[
    "node_modules",
    "target",
    "dist",
    "build",
    ".git",
    "__pycache__",
];

/// Extensions of the files a search reads.
const CODE_EXTENSIONS: &[&str] = &[
    "rs", "py", "js", "ts", "jsx", "tsx", "go", "java", "c", "cpp", "h", "hpp", "rb", "php",
    "swift", "kt", "scala", "cs", "fs", "ex", "exs", "erl", "gleam", "hs", "ml", "sql", "sh",
    "bash", "zsh", "yaml", "yml", "json", "toml", "xml", "html", "css", "scss", "sass", "md",
    "txt",
];

/// Search for a literal pattern in code files.
///
/// The search runs ripgrep, started like every other child a tool starts:
/// with the context's environment and nothing else, so `rg` is looked for
/// on that environment's `PATH`, and with `--no-config`, so no ripgrep
/// configuration file can widen what it reads. When `rg` cannot be started
/// that way, or does not finish, the tree is walked instead, passing over
/// hidden entries, build trees and links.
pub struct SearchCodeTool;

#[async_trait]
impl Tool for SearchCodeTool {
    fn name(&self) -> &str {
        "search_code"
    }

    fn description(&self) -> &str {
        "Search for a literal pattern in code files. Uses ripgrep when available, otherwise walks the tree. Returns matching lines with file paths and line numbers."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Text pattern to search for"
                },
                "path": {
                    "type": "string",
                    "description": "Directory to search in (relative to working directory, default: current directory)"
                },
                "case_sensitive": {
                    "type": "boolean",
                    "description": "If true, search is case-sensitive (default: false)"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of results to return (default 100, max 100)"
                }
            },
            "required": ["pattern"]
        })
    }

    async fn execute(
        &self,
        parameters: Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let parameters: SearchCodeParameters = serde_json::from_value(parameters)
            .map_err(|error| ToolError::InvalidParameters(error.to_string()))?;

        let search_path = resolve(&match &parameters.path {
            Some(path) => context.working_directory.join(path),
            None => context.working_directory.clone(),
        });
        confine(&search_path, context)?;
        let maximum_results = parameters
            .maximum_results
            .unwrap_or(MAXIMUM_SEARCH_RESULTS)
            .min(MAXIMUM_SEARCH_RESULTS);

        if let Some(result) =
            search_ripgrep(&parameters, &search_path, maximum_results, context).await
        {
            return Ok(result);
        }

        let context = context.clone();
        tokio::task::spawn_blocking(move || {
            let (results, stopped) = search_tree(
                &search_path,
                &parameters.pattern,
                parameters.case_sensitive,
                maximum_results,
                &context,
            );
            format_search_results(results, maximum_results, stopped)
        })
        .await
        .map_err(|error| ToolError::Execution(format!("The search did not finish: {error}")))
    }
}

fn format_search_results(
    results: Vec<String>,
    maximum_results: usize,
    stopped: Option<&'static str>,
) -> ToolResult {
    let mut output = if results.is_empty() {
        "No matches found".to_string()
    } else {
        let truncated = if results.len() >= maximum_results {
            format!("\n\n... (truncated at {maximum_results} results)")
        } else {
            String::new()
        };
        format!(
            "Found {} matches:\n\n{}{truncated}",
            results.len(),
            results.join("\n"),
        )
    };
    if let Some(reason) = stopped {
        output.push_str(&format!("\n[search stopped early: {reason}]"));
    }
    ToolResult::success(output)
}

/// Lines under `root` holding `pattern`, as `path:line: text`, and why the
/// walk stopped short of the whole tree if it did.
///
/// Hidden entries, build trees and links are passed over, as is any file past
/// the context's `maximum_file_size`, and every file is opened through the working
/// directory's own descriptor.
pub(super) fn search_tree(
    root: &Path,
    pattern: &str,
    case_sensitive: bool,
    maximum_results: usize,
    context: &ToolContext,
) -> (Vec<String>, Option<&'static str>) {
    let pattern = if case_sensitive {
        pattern.to_string()
    } else {
        pattern.to_lowercase()
    };
    let mut results = Vec::new();
    let mut walk = Walk::new(WALK_TIME_LIMIT);
    let _ = walk.run(root, |entry, file_type| {
        if results.len() >= maximum_results {
            return Visit::Stop;
        }
        let name = entry.file_name();
        let name = name.to_str().unwrap_or_default();
        if name.starts_with('.') {
            return Visit::Skip;
        }
        if file_type.is_dir() {
            return match SKIPPED_DIRECTORIES.contains(&name) {
                true => Visit::Skip,
                false => Visit::Descend,
            };
        }
        if file_type.is_file() {
            search_file(
                &entry.path(),
                root,
                &pattern,
                case_sensitive,
                &mut results,
                maximum_results,
                context,
            );
        }
        Visit::Skip
    });
    (results, walk.stopped())
}

fn search_file(
    path: &Path,
    root: &Path,
    pattern: &str,
    case_sensitive: bool,
    results: &mut Vec<String>,
    maximum_results: usize,
    context: &ToolContext,
) {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default();
    if !CODE_EXTENSIONS.contains(&extension) {
        return;
    }
    let Ok(content) = read_text(context, path) else {
        return;
    };

    let relative = path.strip_prefix(root).unwrap_or(path);
    for (index, line) in content.lines().enumerate() {
        if results.len() >= maximum_results {
            return;
        }
        let matches = if case_sensitive {
            line.contains(pattern)
        } else {
            line.to_lowercase().contains(pattern)
        };
        if matches {
            results.push(format!(
                "{}:{}: {}",
                relative.display(),
                index + 1,
                line.trim()
            ));
        }
    }
}

/// Search with `rg` started through `process::command`. `None`, when it
/// cannot be started or does not finish, hands the search to the walk.
async fn search_ripgrep(
    parameters: &SearchCodeParameters,
    search_path: &Path,
    maximum_results: usize,
    context: &ToolContext,
) -> Option<ToolResult> {
    let mut command = process::command(RIPGREP, context);
    command.args(ripgrep_arguments(parameters, search_path, maximum_results));
    ripgrep(command, search_path, maximum_results, WALK_TIME_LIMIT).await
}

/// Always `--no-config`: a configuration file can add any flag, `--hidden`,
/// `--follow` and `--pre` among them, and so change what a search reads.
fn ripgrep_arguments(
    parameters: &SearchCodeParameters,
    search_path: &Path,
    maximum_results: usize,
) -> Vec<OsString> {
    let mut arguments: Vec<OsString> = [
        "--no-config",
        "-F",
        "-n",
        "--no-heading",
        "--color",
        "never",
        "--max-columns",
        RIPGREP_MAXIMUM_COLUMNS,
        "--max-columns-preview",
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    for skipped in SKIPPED_DIRECTORIES {
        arguments.push("--glob".into());
        arguments.push(format!("!{skipped}/**").into());
    }
    if !parameters.case_sensitive {
        arguments.push("-i".into());
    }
    if maximum_results > 0 {
        arguments.push("-m".into());
        arguments.push(maximum_results.to_string().into());
    }
    arguments.push("--".into());
    arguments.push(parameters.pattern.clone().into());
    arguments.push(search_path.into());
    arguments
}

/// Read `command`'s matches as they arrive, and stop it once
/// `maximum_results` lines are in or `limit` has passed.
///
/// Nothing is buffered beyond the lines kept: a search that matches every
/// line of a large tree costs `maximum_results` lines, not the whole of its
/// output. `None` hands the search to the walk instead.
async fn ripgrep(
    mut command: Command,
    search_path: &Path,
    maximum_results: usize,
    limit: Duration,
) -> Option<ToolResult> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .ok()?;
    let mut stdout = BufReader::new(child.stdout.take()?);
    let deadline = Instant::now() + limit;

    let mut results = Vec::new();
    let mut stopped = None;
    let mut line = Vec::new();
    let finished = loop {
        if results.len() >= maximum_results {
            break false;
        }
        line.clear();
        match timeout_at(deadline, stdout.read_until(b'\n', &mut line)).await {
            Ok(Ok(0)) => break true,
            Ok(Ok(_)) => {
                let text = String::from_utf8_lossy(&line);
                let text = text.trim_end_matches(['\n', '\r']);
                if !text.is_empty() {
                    results.push(normalize_ripgrep_line(text, search_path));
                }
            }
            Ok(Err(_)) => return None,
            Err(_) => {
                stopped = Some("out of time");
                break false;
            }
        }
    };

    if !finished {
        let _ = child.start_kill();
        let _ = child.wait().await;
        return Some(format_search_results(results, maximum_results, stopped));
    }
    let status = timeout_at(deadline, child.wait()).await.ok()?.ok()?;
    if !status.success() && status.code() != Some(RIPGREP_NO_MATCHES) {
        return None;
    }
    Some(format_search_results(results, maximum_results, None))
}

/// A line ripgrep printed as `path:line:text`, with the path made relative
/// to the search root.
fn normalize_ripgrep_line(line: &str, search_path: &Path) -> String {
    let Some((path_and_line, text)) = line.split_once(':').and_then(|(path, rest)| {
        rest.split_once(':')
            .map(|(number, text)| (format!("{path}:{number}"), text))
    }) else {
        return line.to_string();
    };
    let Some((path, number)) = path_and_line.rsplit_once(':') else {
        return format!("{}: {}", path_and_line, text.trim());
    };
    let relative = Path::new(path)
        .strip_prefix(search_path)
        .unwrap_or(Path::new(path));
    format!("{}:{}: {}", relative.display(), number, text.trim())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    use tempfile::TempDir;

    use super::*;
    use crate::test_support::CHILD_TEST;
    use crate::test_support::assert_passed;

    /// The variable ripgrep reads the path of its configuration file from.
    const CONFIGURATION: &str = "RIPGREP_CONFIG_PATH";

    /// Where the stand-in `rg` writes down how it was started, handed to the
    /// test's re-run.
    const RECORD: &str = "ABNEGATE_AGENT_TEST_RIPGREP_RECORD";

    /// The stand-in's file of arguments, one start to a line.
    const ARGUMENTS: &str = "arguments";

    /// The stand-in's file of environments, every variable of every start.
    const ENVIRONMENT: &str = "environment";

    const MARKER: &str = "open sesame please";

    fn shell(line: &str) -> Command {
        let mut command = Command::new("sh");
        command.arg("-c").arg(line);
        command
    }

    /// A ripgrep configuration file in `directory` asking for dot-files,
    /// which the walk passes over and a search has to as well.
    fn hidden(directory: &Path) -> PathBuf {
        let path = directory.join("ripgreprc");
        fs::write(&path, "--hidden\n").expect("the configuration is written");
        path
    }

    /// An `rg` in `directory` that writes its arguments and its environment
    /// beside itself, and matches nothing.
    fn stand_in(directory: &Path) {
        let program = directory.join(RIPGREP);
        fs::write(
            &program,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\n/usr/bin/env >> '{}'\nexit {RIPGREP_NO_MATCHES}\n",
                directory.join(ARGUMENTS).display(),
                directory.join(ENVIRONMENT).display(),
            ),
        )
        .expect("the stand-in is written");
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755))
            .expect("the stand-in is made executable");
    }

    /// This process's path with `directory` searched first.
    fn first_on_path(directory: &Path) -> OsString {
        let path = std::env::var_os("PATH").unwrap_or_default();
        std::env::join_paths(
            std::iter::once(directory.to_path_buf()).chain(std::env::split_paths(&path)),
        )
        .expect("the path joins")
    }

    /// Whether an `rg` is on this process's path, for a test of what the real
    /// ripgrep reads.
    fn installed() -> bool {
        std::env::var_os("PATH").is_some_and(|path| {
            std::env::split_paths(&path).any(|directory| directory.join(RIPGREP).is_file())
        })
    }

    /// A search started ripgrep with this process's whole environment, so a
    /// host's `RIPGREP_CONFIG_PATH` could hand it any flag: `--hidden`,
    /// `--follow`, a `--pre` program. A stand-in `rg` first on the path
    /// writes down every start, so this holds whether ripgrep is installed
    /// or not.
    #[tokio::test]
    async fn ripgrep_starts_with_the_context_environment_and_no_configuration() {
        const NAME: &str = "tool::file::search::tests::ripgrep_starts_with_the_context_environment_and_no_configuration";
        if std::env::var(CHILD_TEST).as_deref() != Ok(NAME) {
            let record = TempDir::new().expect("a directory for the stand-in");
            stand_in(record.path());
            let output = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", NAME, "--nocapture"])
                .env(CHILD_TEST, NAME)
                .env(RECORD, record.path())
                .env(CONFIGURATION, hidden(record.path()))
                .env("PATH", first_on_path(record.path()))
                .output()
                .await
                .unwrap();
            assert_passed(&output);
            return;
        }
        let record = PathBuf::from(std::env::var_os(RECORD).expect("the parent names the record"));
        let tree = TempDir::new().expect("a tree to search");
        fs::write(tree.path().join("visible.rs"), MARKER).expect("a file to search");

        SearchCodeTool
            .execute(
                json!({"pattern": MARKER}),
                &ToolContext::default().within(tree.path()),
            )
            .await
            .expect("the search answers");

        let starts = fs::read_to_string(record.join(ARGUMENTS)).expect("the search started rg");
        assert!(!starts.is_empty(), "the search never started rg");
        for arguments in starts.lines() {
            assert!(
                arguments
                    .split(' ')
                    .any(|argument| argument == "--no-config"),
                "rg was free to read a configuration file: {arguments}"
            );
        }
        let environment = fs::read_to_string(record.join(ENVIRONMENT)).expect("rg's environment");
        assert!(
            !environment
                .lines()
                .any(|line| line.starts_with(&format!("{CONFIGURATION}="))),
            "rg was handed the host's configuration: {environment}"
        );
    }

    /// With a host configuration asking for `--hidden`, ripgrep searched the
    /// dot-files the walk passes over and handed back what they held, even
    /// through a context that passes the host's whole environment on.
    #[tokio::test]
    async fn a_host_ripgrep_configuration_never_widens_a_search() {
        const NAME: &str =
            "tool::file::search::tests::a_host_ripgrep_configuration_never_widens_a_search";
        if std::env::var(CHILD_TEST).as_deref() != Ok(NAME) {
            let configuration = TempDir::new().expect("a directory for the configuration");
            let output = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", NAME, "--nocapture"])
                .env(CHILD_TEST, NAME)
                .env(CONFIGURATION, hidden(configuration.path()))
                .output()
                .await
                .unwrap();
            assert_passed(&output);
            return;
        }
        if !installed() {
            eprintln!("skipping: ripgrep is not installed");
            return;
        }
        let tree = TempDir::new().expect("a tree to search");
        fs::write(tree.path().join(".hidden.rs"), MARKER).expect("a dot-file");
        fs::write(tree.path().join("visible.data"), MARKER).expect("a file only rg reads");

        for context in [
            ToolContext::default().within(tree.path()),
            ToolContext::default()
                .within(tree.path())
                .inherit_environment(),
        ] {
            let output = SearchCodeTool
                .execute(json!({"pattern": MARKER}), &context)
                .await
                .expect("the search answers")
                .output
                .unwrap_or_default();
            assert!(
                output.contains("visible.data"),
                "rg did not run the search: {output}"
            );
            assert!(
                !output.contains(".hidden.rs"),
                "a host configuration widened the search: {output}"
            );
        }
    }

    /// A search matching every line of a huge tree used to buffer every
    /// match ripgrep printed before keeping the first hundred. This one never
    /// stops printing, so only a reader that stops it returns at all.
    #[tokio::test]
    async fn a_search_stops_reading_once_it_has_its_results() {
        let started = std::time::Instant::now();
        let result = ripgrep(
            shell("while :; do echo 'src/a.rs:1:match'; done"),
            Path::new("src"),
            5,
            Duration::from_secs(30),
        )
        .await
        .expect("the reader keeps what it read");

        let output = result.output.unwrap();
        assert!(output.starts_with("Found 5 matches"), "{output}");
        assert!(output.contains("truncated at 5 results"), "{output}");
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[tokio::test]
    async fn a_search_that_runs_out_of_time_reports_what_it_found() {
        let started = std::time::Instant::now();
        let result = ripgrep(
            shell("echo 'a.rs:3:first'; exec sleep 30"),
            Path::new("."),
            MAXIMUM_SEARCH_RESULTS,
            Duration::from_millis(300),
        )
        .await
        .expect("a search out of time still answers");

        let output = result.output.unwrap();
        assert!(output.contains("a.rs:3: first"), "{output}");
        assert!(
            output.contains("search stopped early: out of time"),
            "{output}"
        );
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[tokio::test]
    async fn a_search_that_fails_hands_over_to_the_walk() {
        let result = ripgrep(
            shell("exit 2"),
            Path::new("."),
            MAXIMUM_SEARCH_RESULTS,
            Duration::from_secs(5),
        )
        .await;

        assert!(result.is_none());
    }
}
