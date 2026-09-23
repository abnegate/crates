mod parameters;

use std::ffi::OsStr;
use std::ffi::OsString;
use std::io::Read;
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
use super::resolve;
use super::walk::Visit;
use super::walk::WALK_TIME_LIMIT;
use super::walk::Walk;
use crate::tools::Tool;
use crate::tools::ToolContext;
use crate::tools::ToolError;
use crate::tools::ToolResult;
use crate::tools::beneath;
use crate::tools::beneath::Access;

pub(super) const SEARCH_MAX_RESULTS: usize = 100;

const RIPGREP: &str = "rg";

/// What ripgrep exits with when it searched everything and matched nothing.
const RIPGREP_NO_MATCHES: i32 = 1;

/// Widest matching line ripgrep prints whole; a wider one is cut to a
/// preview, so one minified file cannot fill a result.
const RIPGREP_MAX_COLUMNS: &str = "400";

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

/// Search for code patterns in files
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
        let max_results = parameters
            .max_results
            .unwrap_or(SEARCH_MAX_RESULTS)
            .min(SEARCH_MAX_RESULTS);

        if let Some(result) = search_ripgrep(&parameters, &search_path, max_results).await {
            return Ok(result);
        }

        let context = context.clone();
        tokio::task::spawn_blocking(move || {
            let (results, stopped) = search_tree(
                &search_path,
                &parameters.pattern,
                parameters.case_sensitive,
                max_results,
                &context,
            );
            format_search_results(results, max_results, stopped)
        })
        .await
        .map_err(|error| ToolError::Execution(format!("The search did not finish: {error}")))
    }
}

fn format_search_results(
    results: Vec<String>,
    max_results: usize,
    stopped: Option<&'static str>,
) -> ToolResult {
    let mut output = if results.is_empty() {
        "No matches found".to_string()
    } else {
        let truncated = if results.len() >= max_results {
            format!("\n\n... (truncated at {max_results} results)")
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
/// Hidden entries, build trees and links are passed over, and every file is
/// opened through the working directory's own descriptor.
pub(super) fn search_tree(
    root: &Path,
    pattern: &str,
    case_sensitive: bool,
    max_results: usize,
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
        if results.len() >= max_results {
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
                max_results,
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
    max_results: usize,
    context: &ToolContext,
) {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default();
    if !CODE_EXTENSIONS.contains(&extension) {
        return;
    }
    let Ok(mut file) = beneath::open(context, path, Access::Read) else {
        return;
    };
    let mut content = String::new();
    if file.read_to_string(&mut content).is_err() {
        return;
    }

    let relative = path.strip_prefix(root).unwrap_or(path);
    for (index, line) in content.lines().enumerate() {
        if results.len() >= max_results {
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

async fn search_ripgrep(
    parameters: &SearchCodeParameters,
    search_path: &Path,
    max_results: usize,
) -> Option<ToolResult> {
    if !ripgrep_available() {
        return None;
    }
    let arguments = ripgrep_arguments(parameters, search_path, max_results);
    ripgrep(
        OsStr::new(RIPGREP),
        &arguments,
        search_path,
        max_results,
        WALK_TIME_LIMIT,
    )
    .await
}

fn ripgrep_arguments(
    parameters: &SearchCodeParameters,
    search_path: &Path,
    max_results: usize,
) -> Vec<OsString> {
    let mut arguments: Vec<OsString> = [
        "-F",
        "-n",
        "--no-heading",
        "--color",
        "never",
        "--max-columns",
        RIPGREP_MAX_COLUMNS,
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
    if max_results > 0 {
        arguments.push("-m".into());
        arguments.push(max_results.to_string().into());
    }
    arguments.push("--".into());
    arguments.push(parameters.pattern.clone().into());
    arguments.push(search_path.into());
    arguments
}

/// Read `program`'s matches as they arrive, and stop it once `max_results`
/// lines are in or `limit` has passed.
///
/// Nothing is buffered beyond the lines kept: a search that matches every
/// line of a large tree costs `max_results` lines, not the whole of its
/// output. `None` hands the search to the walk instead.
async fn ripgrep(
    program: &OsStr,
    arguments: &[OsString],
    search_path: &Path,
    max_results: usize,
    limit: Duration,
) -> Option<ToolResult> {
    let mut child = Command::new(program)
        .args(arguments)
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
        if results.len() >= max_results {
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
        return Some(format_search_results(results, max_results, stopped));
    }
    let status = timeout_at(deadline, child.wait()).await.ok()?.ok()?;
    if !status.success() && status.code() != Some(RIPGREP_NO_MATCHES) {
        return None;
    }
    Some(format_search_results(results, max_results, None))
}

fn normalize_ripgrep_line(line: &str, search_path: &Path) -> String {
    // rg prints `path:line:text`. Prefer a path relative to the search root.
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

fn ripgrep_available() -> bool {
    static AVAILABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        std::process::Command::new(RIPGREP)
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shell(line: &str) -> Vec<OsString> {
        vec!["-c".into(), line.into()]
    }

    /// A search matching every line of a huge tree used to buffer every
    /// match ripgrep printed before keeping the first hundred. This one never
    /// stops printing, so only a reader that stops it returns at all.
    #[tokio::test]
    async fn a_search_stops_reading_once_it_has_its_results() {
        let started = std::time::Instant::now();
        let result = ripgrep(
            OsStr::new("sh"),
            &shell("while :; do echo 'src/a.rs:1:match'; done"),
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
            OsStr::new("sh"),
            &shell("echo 'a.rs:3:first'; exec sleep 30"),
            Path::new("."),
            SEARCH_MAX_RESULTS,
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
            OsStr::new("sh"),
            &shell("exit 2"),
            Path::new("."),
            SEARCH_MAX_RESULTS,
            Duration::from_secs(5),
        )
        .await;

        assert!(result.is_none());
    }
}
