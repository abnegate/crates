use std::path::PathBuf;

/// The shell `run_shell` and a backgrounded shell line run through.
pub const SHELL: &str = "sh";

/// The flag that hands [`SHELL`] a command line to run.
pub const SHELL_COMMAND_FLAG: &str = "-c";

/// What a background job runs, and where.
///
/// A program and its arguments rather than a shell line, so that backgrounding
/// keeps `run_command`'s allow-list and metacharacter checks meaning what they
/// mean in the foreground: an argument inspected whole must not be word-split
/// on its way to a child.
///
/// The directory is the child's, and carried here rather than passed beside
/// the session's own tree so that the only path [`Jobs::spawn`](super::Jobs::spawn) takes is the
/// tree it keys a job's log to. A caller naming a directory the model chose
/// can move the child and nothing else.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct JobCommand {
    /// The program to run.
    pub program: String,
    /// Its arguments, each passed whole.
    pub arguments: Vec<String>,
    /// Where the child runs, when not in the session's own working tree.
    pub directory: Option<PathBuf>,
}

impl JobCommand {
    /// A shell line, as `run_shell` takes it.
    pub fn shell(line: impl Into<String>) -> Self {
        Self {
            program: SHELL.to_string(),
            arguments: vec![SHELL_COMMAND_FLAG.to_string(), line.into()],
            directory: None,
        }
    }

    /// A program and its arguments, as `run_command` takes them.
    pub fn new(program: impl Into<String>, arguments: Vec<String>) -> Self {
        Self {
            program: program.into(),
            arguments,
            directory: None,
        }
    }

    /// Run the child somewhere other than the session's own working tree.
    pub fn within(mut self, directory: impl Into<PathBuf>) -> Self {
        self.directory = Some(directory.into());
        self
    }
}
