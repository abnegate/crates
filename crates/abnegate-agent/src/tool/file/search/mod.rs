mod parameters;

use std::ffi::OsStr;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::path::PathBuf;
use std::process::ExitStatus;
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use parameters::SearchCodeParameters;
use serde_json::Value;
use serde_json::json;
use tokio::io::AsyncBufRead;
use tokio::io::AsyncBufReadExt;
use tokio::io::BufReader;
use tokio::process::Command;
use tokio::time::Instant;
use tokio::time::timeout_at;

use super::confine;
use super::read_text;
use super::resolve;
use super::walk::OUT_OF_TIME;
use super::walk::Visit;
use super::walk::Walk;
use crate::tool::TIMEOUT_SLACK;
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

/// Why a search reports only part of the tree when ripgrep fails after
/// printing matches, a file or directory it could not read making it exit 2
/// once it has searched the rest, or when the walk could not read a
/// directory or a code file.
const UNREADABLE: &str = "some files could not be read";

/// Build output and dependency trees a search walks past.
const SKIPPED_DIRECTORIES: &[&str] = &[
    "node_modules",
    "target",
    "dist",
    "build",
    ".git",
    "__pycache__",
];

/// Extensions of the files a walk reads; ripgrep reads every file.
const CODE_EXTENSIONS: &[&str] = &[
    "rs", "py", "js", "ts", "jsx", "tsx", "go", "java", "c", "cpp", "h", "hpp", "rb", "php",
    "swift", "kt", "scala", "cs", "fs", "ex", "exs", "erl", "gleam", "hs", "ml", "sql", "sh",
    "bash", "zsh", "yaml", "yml", "json", "toml", "xml", "html", "css", "scss", "sass", "md",
    "txt",
];

/// Search for a literal pattern in code files.
///
/// The search runs ripgrep with the context's environment and nothing else,
/// as every child a tool starts is run, so the `rg` that runs is the first on
/// that environment's `PATH`. What it reads is then decided by the searched
/// directory alone, through the `.gitignore`, `.ignore`, `.rgignore` and
/// `.git/info/exclude` files within it. Nothing outside it has a say: not a
/// ripgrep configuration file, an ignore file in a directory above, git's
/// global excludes, the exclude file a linked worktree shares with its
/// repository, or whether a repository encloses it.
///
/// Each match is shown only when the file `rg` names for it lies under the
/// searched directory and passes the same confinement every other file tool
/// applies, judged once per file.
///
/// When `rg` cannot be started, or fails having printed nothing, the tree is
/// walked instead. The walk searches code files only, those with a source,
/// script, markup, configuration or text extension such as `rs`, `sh`,
/// `json`, `md` or `txt`, and passes over hidden entries, build trees, links
/// and files past the context's `maximum_file_size`. A search that runs past
/// the context's [`search_timeout`](ToolContext::search_timeout), that `rg`
/// could not read all of, or whose walk could not read a directory or a code
/// file, returns the matches it found, marked as stopped early.
pub struct SearchCodeTool;

#[async_trait]
impl Tool for SearchCodeTool {
    fn name(&self) -> &str {
        "search_code"
    }

    fn description(&self) -> &str {
        "Search for a literal pattern in code files. Uses ripgrep when available, otherwise walks the tree, reading only files with a code extension. Returns matching lines with file paths and line numbers."
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

    fn timeout(&self, context: &ToolContext) -> Duration {
        context.search_timeout + TIMEOUT_SLACK
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
/// Only code files are searched: a file whose extension is not one of
/// [`CODE_EXTENSIONS`] is never read. Hidden entries, build trees and links
/// are passed over, as is any file past the context's `maximum_file_size`,
/// and every file is opened through the working directory's own descriptor.
/// A directory, or a code file, that could not be read is left out, and the
/// search says so with [`UNREADABLE`], as a search through `rg` does; so
/// does a search whose `root` itself could not be read.
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
    let mut unreadable = false;
    let mut walk = Walk::new(context.search_timeout, context);
    let walked = walk.run(root, |entry, file_type| {
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
            unreadable |= !search_file(
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
    let unreadable = unreadable || walk.unreadable() || walked.is_err();
    (results, walk.stopped().or(unreadable.then_some(UNREADABLE)))
}

/// Add the lines of the file at `path` holding `pattern` to `results`.
/// Returns false only when it is a code file within the size limit that
/// could not be read.
fn search_file(
    path: &Path,
    root: &Path,
    pattern: &str,
    case_sensitive: bool,
    results: &mut Vec<String>,
    maximum_results: usize,
    context: &ToolContext,
) -> bool {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default();
    if !CODE_EXTENSIONS.contains(&extension) {
        return true;
    }
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.len() > context.maximum_file_size as u64 => return true,
        Ok(_) => {}
        Err(_) => return false,
    }
    let Ok(content) = read_text(context, path) else {
        return false;
    };

    let relative = escaped(path.strip_prefix(root).unwrap_or(path));
    for (index, line) in content.lines().enumerate() {
        if results.len() >= maximum_results {
            break;
        }
        let matches = if case_sensitive {
            line.contains(pattern)
        } else {
            line.to_lowercase().contains(pattern)
        };
        if matches {
            results.push(format!("{relative}:{}: {}", index + 1, line.trim()));
        }
    }
    true
}

/// Search with `rg` started through `process::command`. `None`, when it
/// cannot be started or fails having printed nothing, hands the search to
/// the walk.
async fn search_ripgrep(
    parameters: &SearchCodeParameters,
    search_path: &Path,
    maximum_results: usize,
    context: &ToolContext,
) -> Option<ToolResult> {
    let mut command = process::command(RIPGREP, context);
    command.args(ripgrep_arguments(parameters, search_path, maximum_results));
    ripgrep(command, search_path, maximum_results, context).await
}

/// The first four keep the host out of what a search reads: `--no-config` a
/// configuration file, which can add any flag, `--hidden`, `--follow` and
/// `--pre` among them; `--no-ignore-parent` the ignore files of the
/// directories above the searched one; `--no-ignore-global` git's global
/// excludes; and `--no-require-git` both whether a repository encloses the
/// searched directory, which otherwise decides whether its `.gitignore` files
/// count, and the exclude file a linked worktree shares with its repository.
///
/// `--null` ends each path with a NUL, which no file name can hold, so a name
/// holding a `:` or a newline is read whole, and `--with-filename` keeps the
/// path when the searched path is a single file.
fn ripgrep_arguments(
    parameters: &SearchCodeParameters,
    search_path: &Path,
    maximum_results: usize,
) -> Vec<OsString> {
    let mut arguments: Vec<OsString> = [
        "--no-config",
        "--no-ignore-parent",
        "--no-ignore-global",
        "--no-require-git",
        "--null",
        "--with-filename",
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
/// `maximum_results` are in or the context's `search_timeout` has passed.
///
/// Nothing is buffered beyond the matches kept: a search that matches every
/// line of a large tree costs `maximum_results` lines, not the whole of its
/// output. Whatever it found is reported, marked when it ran out of time or
/// failed; only a failure that found nothing returns `None`, handing the
/// search to the walk.
async fn ripgrep(
    mut command: Command,
    search_path: &Path,
    maximum_results: usize,
    context: &ToolContext,
) -> Option<ToolResult> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .ok()?;
    let mut stdout = BufReader::new(child.stdout.take()?);
    let deadline = Instant::now() + context.search_timeout;

    let mut results = Vec::new();
    let mut judged: Option<(PathBuf, bool)> = None;
    let mut name = Vec::new();
    let mut found = Vec::new();
    let stopped = loop {
        if results.len() >= maximum_results {
            break None;
        }
        match timeout_at(deadline, next_match(&mut stdout, &mut name, &mut found)).await {
            Ok(Ok(false)) => {
                break match timeout_at(deadline, child.wait()).await {
                    Ok(Ok(status)) if searched_everything(status) => None,
                    Ok(_) => Some(UNREADABLE),
                    Err(_) => Some(OUT_OF_TIME),
                };
            }
            Ok(Ok(true)) => {
                let path = Path::new(OsStr::from_bytes(&name));
                let shown = match &judged {
                    Some((last, shown)) if last == path => *shown,
                    _ => {
                        let shown = reachable(path, search_path, context);
                        judged = Some((path.to_path_buf(), shown));
                        shown
                    }
                };
                if shown {
                    results.push(shown_match(
                        path,
                        &String::from_utf8_lossy(&found),
                        search_path,
                    ));
                }
            }
            Ok(Err(_)) => break Some(UNREADABLE),
            Err(_) => break Some(OUT_OF_TIME),
        }
    };

    let _ = child.start_kill();
    let _ = child.wait().await;
    if results.is_empty() && stopped == Some(UNREADABLE) {
        return None;
    }
    Some(format_search_results(results, maximum_results, stopped))
}

/// Read ripgrep's next `path\0number:text` record into `name` and `found`,
/// or return `false` once its output has ended.
///
/// The path runs to the NUL, so it is read whole whatever it holds, and what
/// follows runs to the end of the line, which a matched line cannot hold. A
/// record cut off before its NUL, such as the notice ripgrep prints for a
/// binary file it was handed, ends the output.
async fn next_match<Reader: AsyncBufRead + Unpin>(
    stdout: &mut Reader,
    name: &mut Vec<u8>,
    found: &mut Vec<u8>,
) -> io::Result<bool> {
    name.clear();
    found.clear();
    stdout.read_until(b'\0', name).await?;
    if name.pop() != Some(b'\0') {
        return Ok(false);
    }
    stdout.read_until(b'\n', found).await?;
    Ok(true)
}

/// Whether a match in the file ripgrep named `path` may be shown: only a
/// path beneath the searched directory, which is all ripgrep was handed, and
/// only when it stays confined once resolved.
fn reachable(path: &Path, search_path: &Path, context: &ToolContext) -> bool {
    path.is_absolute() && path.starts_with(search_path) && confine(&resolve(path), context).is_ok()
}

/// Whether ripgrep exited having searched all it was given, matching or not.
fn searched_everything(status: ExitStatus) -> bool {
    status.success() || status.code() == Some(RIPGREP_NO_MATCHES)
}

/// A match as the model reads it, `path:number: text`, the path relative to
/// the search root unless the root is the file itself.
fn shown_match(path: &Path, found: &str, search_path: &Path) -> String {
    let shown = match path.strip_prefix(search_path) {
        Ok(relative) if !relative.as_os_str().is_empty() => relative,
        _ => path,
    };
    let shown = escaped(shown);
    match found.split_once(':') {
        Some((number, text)) => format!("{shown}:{number}: {}", text.trim()),
        None => format!("{shown}: {}", found.trim()),
    }
}

/// `path` with each control character in it escaped, so a name holding a
/// newline stays on its match's line rather than passing for a match of its
/// own.
fn escaped(path: &Path) -> String {
    let mut shown = String::new();
    for character in path.to_string_lossy().chars() {
        if character.is_control() {
            shown.extend(character.escape_default());
        } else {
            shown.push(character);
        }
    }
    shown
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    use tempfile::TempDir;

    use super::*;
    use crate::test_support::CHILD_TEST;
    use crate::test_support::PATIENCE;
    use crate::test_support::TIMEOUT;
    use crate::test_support::assert_passed;
    use crate::test_support::expired;
    use crate::test_support::timed;
    use crate::tool::EnvironmentPolicy;

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

    /// The files a search's output names, in order.
    fn files(output: &str) -> Vec<&str> {
        let mut files: Vec<&str> = output
            .lines()
            .filter_map(|line| line.split_once(':'))
            .filter(|(_, rest)| rest.ends_with(MARKER))
            .map(|(file, _)| file)
            .collect();
        files.sort_unstable();
        files
    }

    /// A tree under `outer`, beneath an ignore file that whitelists one
    /// dot-file and ignores one plain file, holding a file for every rule a
    /// host could bring to a search and a `.gitignore` of its own.
    fn planted(outer: &Path) -> PathBuf {
        fs::write(outer.join(".rgignore"), "!.parent.rs\nparent.data\n")
            .expect("an ignore file above the tree");
        let tree = outer.join("tree");
        fs::create_dir(&tree).expect("a tree to search");
        fs::write(tree.join(".gitignore"), "tree.data\n").expect("the tree's own ignore file");
        for name in [
            ".hidden.rs",
            ".parent.rs",
            ".global.rs",
            ".shared.rs",
            "visible.data",
            "parent.data",
            "global.data",
            "shared.data",
            "tree.data",
        ] {
            fs::write(tree.join(name), MARKER).expect("a file to search");
        }
        tree
    }

    /// Make `tree` a linked worktree of a repository beside it, whose shared
    /// exclude file whitelists one dot-file and ignores one plain file.
    fn link(tree: &Path) {
        let repository = tree.with_file_name("repository");
        let worktree = repository.join("worktrees").join("tree");
        fs::create_dir_all(repository.join("info")).expect("the repository's info directory");
        fs::create_dir_all(&worktree).expect("the worktree's own git directory");
        fs::write(
            repository.join("info").join("exclude"),
            "!.shared.rs\nshared.data\n",
        )
        .expect("the exclude file the worktree shares");
        fs::write(worktree.join("commondir"), "../..\n").expect("the worktree's common directory");
        fs::write(
            tree.join(".git"),
            format!("gitdir: {}\n", worktree.display()),
        )
        .expect("the worktree's pointer to its git directory");
    }

    /// A search started ripgrep with this process's whole environment, so a
    /// host's `RIPGREP_CONFIG_PATH` could hand it any flag: `--hidden`,
    /// `--follow`, a `--pre` program. It also started it free to read the
    /// ignore files above the tree, git's global excludes, and whether a
    /// repository encloses the tree. A stand-in `rg` first on the path
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
            for (flag, freedom) in [
                ("--no-config", "a configuration file"),
                ("--no-ignore-parent", "the ignore files above the tree"),
                ("--no-ignore-global", "git's global excludes"),
                ("--no-require-git", "whether a repository encloses the tree"),
            ] {
                assert!(
                    arguments.split(' ').any(|argument| argument == flag),
                    "rg was started without {flag}, free to read {freedom}: {arguments}"
                );
            }
        }
        let environment = fs::read_to_string(record.join(ENVIRONMENT)).expect("rg's environment");
        assert!(
            !environment
                .lines()
                .any(|line| line.starts_with(&format!("{CONFIGURATION}="))),
            "rg was handed the host's configuration: {environment}"
        );
    }

    /// What a search read used to follow the host as well as the tree. A
    /// ripgrep configuration asking for `--hidden` handed back the dot-files
    /// the walk passes over; an ignore file in a directory above the tree,
    /// git's global excludes, or the exclude file a linked worktree shares
    /// with its repository, whitelisted a dot-file or ignored a file the
    /// search should read; and whether a repository enclosed the tree decided
    /// whether its own `.gitignore` counted. None of it may widen or narrow a
    /// search, even through a context that passes the host's whole
    /// environment on.
    #[tokio::test]
    async fn nothing_on_the_host_widens_or_narrows_a_search() {
        const NAME: &str =
            "tool::file::search::tests::nothing_on_the_host_widens_or_narrows_a_search";
        if std::env::var(CHILD_TEST).as_deref() != Ok(NAME) {
            let configuration = TempDir::new().expect("a directory for the configuration");
            let home = TempDir::new().expect("a home for the child");
            let settings = home.path().join(".config");
            fs::create_dir_all(settings.join("git")).expect("git's settings directory");
            fs::write(
                settings.join("git").join("ignore"),
                "!.global.rs\nglobal.data\n",
            )
            .expect("git's global excludes");
            let output = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", NAME, "--nocapture"])
                .env(CHILD_TEST, NAME)
                .env(CONFIGURATION, hidden(configuration.path()))
                .env("HOME", home.path())
                .env("XDG_CONFIG_HOME", &settings)
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
        let enclosed = TempDir::new().expect("a repository to hold a tree");
        fs::create_dir(enclosed.path().join(".git")).expect("the repository's own directory");
        let alone = TempDir::new().expect("a directory to hold a tree");
        let worktree = TempDir::new().expect("a directory to hold a linked worktree");
        let linked = planted(worktree.path());
        link(&linked);

        for tree in [planted(enclosed.path()), planted(alone.path()), linked] {
            for context in [
                ToolContext::default().within(&tree),
                ToolContext::default().within(&tree).inherit_environment(),
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
                assert_eq!(
                    files(&output),
                    ["global.data", "parent.data", "shared.data", "visible.data"],
                    "the host changed what a search of {} read: {output}",
                    tree.display()
                );
            }
        }
    }

    /// A context rooted at `root` that searches for as long as any test
    /// waits.
    fn patient(root: &Path) -> ToolContext {
        ToolContext::default()
            .within(root)
            .with_search_timeout(TIMEOUT)
    }

    /// A shell line printing one match of ripgrep's `--null` output: `file`
    /// under `root`, then `found`.
    fn printed(root: &Path, file: &str, found: &str) -> String {
        format!(
            "printf '%s\\0%s\\n' '{}' '{found}'",
            root.join(file).display()
        )
    }

    /// A search matching every line of a huge tree used to buffer every
    /// match ripgrep printed before keeping the first hundred. This one never
    /// stops printing, so only a reader that stops it returns at all.
    #[tokio::test]
    async fn a_search_stops_reading_once_it_has_its_results() {
        let (_tree, root) = tree();
        let started = root.join("started");
        let line = format!(
            "touch '{}'; while :; do {}; done",
            started.display(),
            printed(&root, "a.rs", "1:match")
        );

        let (result, waited) =
            timed(ripgrep(shell(&line), &root, 5, &patient(&root)), &started).await;

        let output = result
            .expect("the reader keeps what it read")
            .output
            .unwrap();
        assert!(output.starts_with("Found 5 matches"), "{output}");
        assert!(output.contains("a.rs:1: match"), "{output}");
        assert!(output.contains("truncated at 5 results"), "{output}");
        assert!(waited < PATIENCE, "the reader went on reading: {waited:?}");
    }

    #[tokio::test]
    async fn a_search_that_runs_out_of_time_reports_what_it_found() {
        let (_tree, root) = tree();
        let spoken = root.join("spoken");
        let line = format!(
            "{}; touch '{}'; exec sleep 120",
            printed(&root, "a.rs", "3:first"),
            spoken.display()
        );

        let (result, waited) = expired(
            ripgrep(shell(&line), &root, MAXIMUM_SEARCH_RESULTS, &patient(&root)),
            &spoken,
        )
        .await;

        let output = result
            .expect("a search out of time still answers")
            .output
            .unwrap();
        assert!(output.contains("a.rs:3: first"), "{output}");
        assert!(
            output.contains("search stopped early: out of time"),
            "{output}"
        );
        assert!(
            waited < PATIENCE,
            "the search waited on rg past its limit: {waited:?}"
        );
    }

    /// A `rg` that closed its output but had not exited by the deadline
    /// handed the search to the walk, throwing away every match it printed.
    #[tokio::test]
    async fn a_search_whose_rg_outlives_its_output_reports_what_it_found() {
        let (_tree, root) = tree();
        let spoken = root.join("spoken");
        let line = format!(
            "{}; exec >&-; touch '{}'; exec sleep 120",
            printed(&root, "a.rs", "3:first"),
            spoken.display()
        );

        let (result, waited) = expired(
            ripgrep(shell(&line), &root, MAXIMUM_SEARCH_RESULTS, &patient(&root)),
            &spoken,
        )
        .await;

        let output = result
            .expect("a search out of time still answers")
            .output
            .unwrap();
        assert!(output.contains("a.rs:3: first"), "{output}");
        assert!(
            output.contains("search stopped early: out of time"),
            "{output}"
        );
        assert!(
            waited < PATIENCE,
            "the search waited on rg past its limit: {waited:?}"
        );
    }

    /// `rg` exits 2 when it could not read part of the tree, having still
    /// printed the matches it found in the rest. Those were thrown away for
    /// the walk, which reads fewer kinds of file and passes over what it
    /// cannot read without a word, so the model heard of no matches at all.
    #[tokio::test]
    async fn a_search_that_could_not_read_everything_keeps_what_it_found() {
        let (_tree, root) = tree();
        let line = format!("{}; exit 2", printed(&root, "a.rs", "3:first"));

        let result = ripgrep(shell(&line), &root, MAXIMUM_SEARCH_RESULTS, &patient(&root))
            .await
            .expect("the matches rg printed are kept");

        let output = result.output.unwrap();
        assert!(output.starts_with("Found 1 matches"), "{output}");
        assert!(output.contains("a.rs:3: first"), "{output}");
        assert!(
            output.contains("search stopped early: some files could not be read"),
            "{output}"
        );
    }

    /// Take every permission away from `path`, and say whether that made it
    /// unreadable: a process running as root reads it regardless.
    fn lock(path: &Path) -> bool {
        fs::set_permissions(path, fs::Permissions::from_mode(0o000))
            .expect("the permissions change");
        if path.is_dir() {
            fs::read_dir(path).is_err()
        } else {
            fs::read(path).is_err()
        }
    }

    fn unlock(path: &Path, mode: u32) {
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
            .expect("the permissions are restored");
    }

    /// A tree with one readable match and `locked`, whatever `plant` makes of
    /// it, and what a walk of it given `timeout` found and why it stopped, if
    /// it did. `None` when `locked` could not be made unreadable.
    fn walked(
        locked: &str,
        plant: impl FnOnce(&Path),
        mode: u32,
        timeout: Duration,
    ) -> Option<(Vec<String>, Option<&'static str>)> {
        let tree = TempDir::new().expect("a tree to search");
        fs::write(tree.path().join("found.rs"), MARKER).expect("a file to search");
        let locked = tree.path().join(locked);
        plant(&locked);
        if !lock(&locked) {
            unlock(&locked, mode);
            eprintln!("skipping: {} stayed readable", locked.display());
            return None;
        }

        let walked = search_tree(
            tree.path(),
            MARKER,
            false,
            MAXIMUM_SEARCH_RESULTS,
            &ToolContext::default()
                .within(tree.path())
                .with_search_timeout(timeout),
        );

        unlock(&locked, mode);
        Some(walked)
    }

    /// The walk passed over a file or a directory it could not read without
    /// a word, so a search that missed part of the tree reported what it
    /// found as the whole answer. It now says so, as a search through `rg`
    /// does.
    #[test]
    fn a_walk_that_could_not_read_a_code_file_says_so() {
        let Some((results, stopped)) = walked(
            "locked.rs",
            |path| fs::write(path, MARKER).expect("a file to lock"),
            0o644,
            TIMEOUT,
        ) else {
            return;
        };

        assert_eq!(files(&results.join("\n")), ["found.rs"]);
        assert_eq!(stopped, Some(UNREADABLE));
    }

    #[test]
    fn a_walk_that_could_not_read_a_directory_says_so() {
        let Some((results, stopped)) = walked(
            "closed",
            |path| {
                fs::create_dir(path).expect("a directory to lock");
                fs::write(path.join("inside.rs"), MARKER).expect("a file inside it");
            },
            0o755,
            TIMEOUT,
        ) else {
            return;
        };

        assert_eq!(files(&results.join("\n")), ["found.rs"]);
        assert_eq!(stopped, Some(UNREADABLE));
    }

    /// A root the walk could not open was reported as a tree with no match.
    #[test]
    fn a_walk_that_could_not_read_its_root_says_so() {
        let tree = TempDir::new().expect("a tree to search");
        fs::write(tree.path().join("found.rs"), MARKER).expect("a file to search");
        if !lock(tree.path()) {
            unlock(tree.path(), 0o755);
            eprintln!("skipping: {} stayed readable", tree.path().display());
            return;
        }

        let (results, stopped) = search_tree(
            tree.path(),
            MARKER,
            false,
            MAXIMUM_SEARCH_RESULTS,
            &ToolContext::default().within(tree.path()),
        );

        unlock(tree.path(), 0o755);
        assert!(results.is_empty(), "{results:?}");
        assert_eq!(stopped, Some(UNREADABLE));
    }

    /// The walk searches code files only, so a file with any other extension
    /// was never going to be read, and not reading it misses nothing.
    #[test]
    fn a_walk_passes_over_an_unreadable_file_it_would_not_search_in_silence() {
        let Some((results, stopped)) = walked(
            "locked.bin",
            |path| fs::write(path, MARKER).expect("a file to lock"),
            0o644,
            TIMEOUT,
        ) else {
            return;
        };

        assert_eq!(files(&results.join("\n")), ["found.rs"]);
        assert_eq!(stopped, None);
    }

    /// The walk reads its time from the context's `search_timeout` and says
    /// when it could not read part of the tree: out of time, it reports that
    /// first; within its time, it reports what it could not read.
    #[test]
    fn a_walk_honours_its_search_timeout_and_what_it_could_not_read() {
        let plant = |path: &Path| fs::write(path, MARKER).expect("a file to lock");
        let Some((results, stopped)) = walked("locked.rs", plant, 0o644, Duration::ZERO) else {
            return;
        };
        assert!(results.is_empty(), "{results:?}");
        assert_eq!(stopped, Some(OUT_OF_TIME));

        let Some((results, stopped)) = walked("locked.rs", plant, 0o644, TIMEOUT) else {
            return;
        };
        assert_eq!(files(&results.join("\n")), ["found.rs"]);
        assert_eq!(stopped, Some(UNREADABLE));
    }

    #[tokio::test]
    async fn a_search_that_fails_hands_over_to_the_walk() {
        let (_tree, root) = tree();

        let result = ripgrep(
            shell("exit 2"),
            &root,
            MAXIMUM_SEARCH_RESULTS,
            &patient(&root),
        )
        .await;

        assert!(result.is_none());
    }

    /// An `rg` in `directory` that runs `script`.
    fn impostor(directory: &Path, script: &str) {
        let program = directory.join(RIPGREP);
        fs::write(&program, format!("#!/bin/sh\n{script}\n")).expect("the impostor is written");
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755))
            .expect("the impostor is made executable");
    }

    /// A context rooted at `tree` whose children find only what `directory`
    /// holds on their `PATH`.
    fn impersonated(tree: &Path, directory: &Path) -> ToolContext {
        ToolContext::default().within(tree).with_environment(
            EnvironmentPolicy::empty().with("PATH", directory.display().to_string()),
        )
    }

    /// A temporary tree, and its canonical path.
    fn tree() -> (TempDir, PathBuf) {
        let tree = TempDir::new().expect("a tree to search");
        let root = tree.path().canonicalize().expect("the tree resolves");
        (tree, root)
    }

    async fn searched(parameters: Value, context: &ToolContext) -> String {
        SearchCodeTool
            .execute(parameters, context)
            .await
            .expect("the search answers")
            .output
            .unwrap_or_default()
    }

    /// ripgrep's `path:line:text` was split at its first `:`, so a file
    /// whose name held one had its line number taken from its name.
    #[tokio::test]
    async fn a_match_in_a_file_named_with_a_colon_keeps_its_line() {
        if !installed() {
            eprintln!("skipping: ripgrep is not installed");
            return;
        }
        let (_tree, root) = tree();
        fs::write(root.join("a:1:b.rs"), format!("first\n{MARKER}\n"))
            .expect("a file named with colons");

        let output = searched(
            json!({"pattern": MARKER}),
            &ToolContext::default().within(&root),
        )
        .await;

        assert!(
            output.contains(&format!("a:1:b.rs:2: {MARKER}")),
            "{output}"
        );
    }

    /// ripgrep leaves the path out when it is handed a single file, so every
    /// match of a search of one file came back without the file it was in.
    #[tokio::test]
    async fn a_search_of_one_file_reports_its_path() {
        if !installed() {
            eprintln!("skipping: ripgrep is not installed");
            return;
        }
        let (_tree, root) = tree();
        fs::write(root.join("notes.rs"), format!("first\n{MARKER}\n")).expect("a file to search");

        let output = searched(
            json!({"pattern": MARKER, "path": "notes.rs"}),
            &ToolContext::default().within(&root),
        )
        .await;

        assert!(
            output.contains(&format!("{}:2: {MARKER}", root.join("notes.rs").display())),
            "{output}"
        );
    }

    /// ripgrep's output was cut into lines before the path was read, so a
    /// file named with a newline yielded a match for a path made of the end
    /// of its name, which may be another file or none. Only a path under the
    /// searched directory is reported now, and a newline in a name is shown
    /// escaped.
    #[tokio::test]
    async fn a_file_named_with_a_newline_yields_no_stray_match() {
        let (_tree, root) = tree();
        fs::write(root.join("a\nb.rs"), MARKER).expect("a file named with a newline");
        let stray = |output: &str| output.lines().any(|line| line.starts_with("b.rs:"));

        if installed() {
            let output = searched(
                json!({"pattern": MARKER}),
                &ToolContext::default().within(&root),
            )
            .await;
            assert!(
                output.contains(&format!("a\\nb.rs:1: {MARKER}")),
                "{output}"
            );
            assert!(!stray(&output), "{output}");
        }

        let directory = TempDir::new().expect("a directory for the impostor");
        impostor(
            directory.path(),
            &format!("printf '%s\\0%s\\n' b.rs '1:{MARKER}' /elsewhere/b.rs '1:{MARKER}'"),
        );
        let output = searched(
            json!({"pattern": MARKER}),
            &impersonated(&root, directory.path()),
        )
        .await;
        assert!(output.starts_with("No matches found"), "{output}");
    }

    /// A search ran `rg` for twenty seconds whatever its context said, so a
    /// caller could neither shorten a search nor lengthen one past the
    /// tool's default outer bound.
    #[tokio::test(start_paused = true)]
    async fn a_search_stops_at_the_limit_its_context_sets() {
        const LIMIT: Duration = Duration::from_secs(1);
        let (_tree, root) = tree();
        let directory = TempDir::new().expect("a directory for the impostor");
        impostor(directory.path(), "exec /bin/sleep 120");
        let context = impersonated(&root, directory.path()).with_search_timeout(LIMIT);

        let started = Instant::now();
        let output = searched(json!({"pattern": MARKER}), &context).await;
        let elapsed = started.elapsed();

        assert!(output.contains("out of time"), "{output}");
        assert!(
            elapsed >= LIMIT && elapsed < Duration::from_secs(20),
            "the search ran for {elapsed:?}"
        );
        assert_eq!(
            SearchCodeTool.timeout(&context),
            LIMIT + crate::tool::TIMEOUT_SLACK
        );
    }
}
