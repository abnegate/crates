/// The latest word a deployment's provider gave on how it went.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct DeploymentStatus {
    /// GitHub's word for where it stands, such as `pending`, `in_progress`,
    /// `success`, `failure`, `error` or `inactive`, or empty when GitHub does
    /// not say.
    pub state: String,
    /// What the provider said about it, if anything.
    pub description: Option<String>,
    /// Where what was deployed can be reached, if the provider says.
    pub environment_url: Option<String>,
    /// Where the deployment's output can be read, if the provider says.
    pub log_url: Option<String>,
    /// When the provider said it, as RFC 3339, or empty when GitHub does not
    /// say.
    pub created_at: String,
}

impl DeploymentStatus {
    /// A status in `state`, given at `created_at`, as RFC 3339, with nothing
    /// else said.
    pub fn new(state: impl Into<String>, created_at: impl Into<String>) -> Self {
        Self {
            state: state.into(),
            created_at: created_at.into(),
            ..Self::default()
        }
    }

    /// What the provider said about it.
    #[must_use]
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Where what was deployed can be reached.
    #[must_use]
    pub fn with_environment_url(mut self, environment_url: impl Into<String>) -> Self {
        self.environment_url = Some(environment_url.into());
        self
    }

    /// Where the deployment's output can be read.
    #[must_use]
    pub fn with_log_url(mut self, log_url: impl Into<String>) -> Self {
        self.log_url = Some(log_url.into());
        self
    }
}
