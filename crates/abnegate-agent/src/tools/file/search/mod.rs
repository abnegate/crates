mod parameters;

use async_trait::async_trait;
use serde_json::{Value, json};
use std::io::Read;
use std::path::Path;

use super::walk::{Visit, WALK_TIME_LIMIT, Walk};
use super::{confine, resolve};
use crate::tools::beneath::{self, Access};
use crate::tools::{Tool, ToolContext, ToolError, ToolResult};
use parameters::SearchCodeParameters;

pub(super) const SEARCH_MAX_RESULTS: usize = 100;

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

    let mut command = tokio::process::Command::new("rg");
    command
        .arg("-F")
        .arg("-n")
        .arg("--no-heading")
        .arg("--color")
        .arg("never")
        .arg("--glob")
        .arg("!node_modules/**")
        .arg("--glob")
        .arg("!target/**")
        .arg("--glob")
        .arg("!dist/**")
        .arg("--glob")
        .arg("!build/**")
        .arg("--glob")
        .arg("!__pycache__/**");
    if !parameters.case_sensitive {
        command.arg("-i");
    }
    if max_results > 0 {
        command.arg("-m").arg(max_results.to_string());
    }
    command.arg("--").arg(&parameters.pattern).arg(search_path);
    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::null());

    let output = command.output().await.ok()?;
    // 0 = matches, 1 = no matches; anything else is a real failure.
    if !output.status.success() && output.status.code() != Some(1) {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut results = Vec::new();
    for line in stdout.lines() {
        if results.len() >= max_results {
            break;
        }
        if line.is_empty() {
            continue;
        }
        results.push(normalize_ripgrep_line(line, search_path));
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
        std::process::Command::new("rg")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    })
}
