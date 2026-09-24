use std::path::PathBuf;

use serde::Deserialize;
use serde::Serialize;

use super::process_tree_request::ProcessTreeRequest;

/// Filesystem a confined job is allowed to see.
///
/// Everything outside these roots is denied, as is the network. Roots must be
/// absolute paths that exist when the job starts.
///
/// The [`Default`] grants no root at all and asks for the single-command
/// confinement; [`with_read_roots`](Self::with_read_roots) and
/// [`with_write_roots`](Self::with_write_roots) name what the job may see.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[non_exhaustive]
pub struct ConfinementRequest {
    /// Directories the job may read
    #[serde(default)]
    pub read_roots: Vec<PathBuf>,
    /// Directories the job may read and write
    #[serde(default)]
    pub write_roots: Vec<PathBuf>,
    /// Absent asks for the single-command confinement: the job runs one
    /// executable, which may neither fork nor exec anything else on a runner
    /// that advertises `confinement_single_process`. Present asks for a
    /// bounded process tree, which a build tool needs and a verification
    /// recipe does not, and which only a runner advertising
    /// `confinement_process_tree` accepts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process_tree: Option<ProcessTreeRequest>,
}

impl ConfinementRequest {
    /// Let the job read `roots`, in place of any named before.
    pub fn with_read_roots<I, P>(mut self, roots: I) -> Self
    where
        I: IntoIterator<Item = P>,
        P: Into<PathBuf>,
    {
        self.read_roots = roots.into_iter().map(Into::into).collect();
        self
    }

    /// Let the job read and write `roots`, in place of any named before.
    pub fn with_write_roots<I, P>(mut self, roots: I) -> Self
    where
        I: IntoIterator<Item = P>,
        P: Into<PathBuf>,
    {
        self.write_roots = roots.into_iter().map(Into::into).collect();
        self
    }

    /// Run as a process tree bounded by `process_tree` instead of as a single
    /// command.
    pub fn with_process_tree(mut self, process_tree: ProcessTreeRequest) -> Self {
        self.process_tree = Some(process_tree);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_built_from_its_roots_round_trips() {
        let request = ConfinementRequest::default()
            .with_read_roots(["/source", "/shared"])
            .with_write_roots([PathBuf::from("/source/target")])
            .with_process_tree(ProcessTreeRequest::new(vec![PathBuf::from("/usr/bin")]));

        let line = serde_json::to_string(&request).unwrap();

        assert_eq!(
            line,
            r#"{"read_roots":["/source","/shared"],"write_roots":["/source/target"],"process_tree":{"execute_roots":["/usr/bin"]}}"#
        );
        assert_eq!(
            serde_json::from_str::<ConfinementRequest>(&line).unwrap(),
            request
        );
    }

    #[test]
    fn the_default_grants_no_root_to_a_single_command() {
        let request = ConfinementRequest::default();

        assert!(request.read_roots.is_empty());
        assert!(request.write_roots.is_empty());
        assert!(request.process_tree.is_none());
    }

    #[test]
    fn naming_roots_again_replaces_them() {
        let request = ConfinementRequest::default()
            .with_read_roots(["/first"])
            .with_write_roots(["/first"])
            .with_read_roots(["/second"])
            .with_write_roots(Vec::<PathBuf>::new());

        assert_eq!(request.read_roots, [PathBuf::from("/second")]);
        assert!(request.write_roots.is_empty());
    }
}
