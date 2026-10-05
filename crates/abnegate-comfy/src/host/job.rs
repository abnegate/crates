use crate::host::SCHEMA_VERSION;
use crate::host::Status;
use crate::train::TrainProgress;
use chrono::DateTime;
use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Map;
use serde_json::Value;
use uuid::Uuid;

/// The keys `job.json` gives a meaning of its own. A
/// [field](Job::with_field) a host adds under one of them is dropped.
const KEYS: &[&str] = &[
    "schema_version",
    "id",
    "name",
    "method",
    "subject",
    "trigger",
    "architecture",
    "checkpoint",
    "base",
    "status",
    "filename",
    "recipe_id",
    "hf_base",
    "image_count",
    "error",
    "pid",
    "started_at",
    "published",
    "secrets",
];

/// One host training job, as its `job.json` holds it.
///
/// The crate writes it [`Status::Queued`]; the host trainer moves it to
/// [`Status::Running`] with its [`process_id`](Job::process_id), then to [`Status::Succeeded`] once the
/// weight is in the job directory under [`Job::filename`], or to
/// [`Status::Failed`] with an [`error`](Job::error). Keys this version does not
/// know, such as the ones a trainer stamps for its own dashboard, are kept in
/// [`fields`](Job::fields) and written back unchanged.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Job {
    /// Version of the `job.json` layout, [`SCHEMA_VERSION`].
    pub schema_version: u32,
    /// The job, and the name of its directory.
    pub id: Uuid,
    /// The name the weight was requested under.
    pub name: String,
    /// How the trainer turns the set into a weight, a
    /// [`TrainMethod`](crate::lora::TrainMethod) wire name or one the host's
    /// trainer defines.
    pub method: String,
    /// What the set shows, a [`TrainSubject`](crate::lora::TrainSubject) wire
    /// name or one the host's trainer defines. Blank when not given.
    #[serde(default)]
    pub subject: String,
    /// The word an identity weight is prompted with. Blank when none.
    #[serde(default)]
    pub trigger: String,
    /// The base's model family, such as `flux` or `sdxl`, the same name a
    /// training command reads in `<prefix>_ARCHITECTURE`. Blank when not
    /// given.
    #[serde(default)]
    pub architecture: String,
    /// The base weight file the trainer starts from.
    pub checkpoint: String,
    /// [`Recipe::id`](crate::recipe::Recipe::id) of the base. Blank for a job
    /// a host queued for a trainer of its own, which the crate never
    /// publishes.
    #[serde(default)]
    pub base: String,
    /// Where the job stands.
    pub status: Status,
    /// The file the trainer writes the weight to inside the job directory,
    /// and the name it is published under.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    /// The recipe that runs the published weight.
    #[serde(default)]
    pub recipe_id: String,
    /// The Hugging Face model the trainer loads for the base. Blank when the
    /// base names none.
    #[serde(default, rename = "hf_base")]
    pub huggingface_base: String,
    /// Images in the job's `dataset/`.
    #[serde(default)]
    pub image_count: usize,
    /// Why a [`Status::Failed`] job failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The trainer process holding a [`Status::Running`] job.
    #[serde(default, rename = "pid", skip_serializing_if = "Option::is_none")]
    pub process_id: Option<u32>,
    /// When the job was queued; the latest is the
    /// [current](crate::host::current) one.
    pub started_at: DateTime<Utc>,
    /// Where the crate published the job's weight, relative to the models
    /// directory, once it has. A published job is never published again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published: Option<String>,
    /// The secrets written beside `job.json` with
    /// [`JobDirectory::write_secret`](crate::host::JobDirectory::write_secret),
    /// removed once the job ends.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secrets: Vec<String>,
    /// Every other key of `job.json`, as written.
    #[serde(flatten)]
    pub fields: Map<String, Value>,
    /// The job's `progress.json`, when it was read with
    /// [`current_with_progress`](crate::host::current_with_progress). Never
    /// written to `job.json`.
    #[serde(skip)]
    pub progress: Option<TrainProgress>,
}

impl Job {
    /// A new [`Status::Queued`] job training `name` with `method` from the
    /// base weight `checkpoint`, started now.
    pub fn new(
        name: impl Into<String>,
        method: impl Into<String>,
        checkpoint: impl Into<String>,
    ) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            id: Uuid::new_v4(),
            name: name.into(),
            method: method.into(),
            subject: String::new(),
            trigger: String::new(),
            architecture: String::new(),
            checkpoint: checkpoint.into(),
            base: String::new(),
            status: Status::Queued,
            filename: None,
            recipe_id: String::new(),
            huggingface_base: String::new(),
            image_count: 0,
            error: None,
            process_id: None,
            started_at: Utc::now(),
            published: None,
            secrets: Vec::new(),
            fields: Map::new(),
            progress: None,
        }
    }

    /// Sets what the set shows.
    pub fn with_subject(mut self, subject: impl Into<String>) -> Self {
        self.subject = subject.into();
        self
    }

    /// Sets the word an identity weight is prompted with.
    pub fn with_trigger(mut self, trigger: impl Into<String>) -> Self {
        self.trigger = trigger.into();
        self
    }

    /// Sets the base's model family.
    pub fn with_architecture(mut self, architecture: impl Into<String>) -> Self {
        self.architecture = architecture.into();
        self
    }

    /// Sets the base recipe, which makes the job one the crate publishes.
    pub fn with_base(mut self, base: impl Into<String>) -> Self {
        self.base = base.into();
        self
    }

    /// Sets the file the trainer writes the weight to.
    pub fn with_filename(mut self, filename: impl Into<String>) -> Self {
        self.filename = Some(filename.into());
        self
    }

    /// Sets the recipe that runs the published weight.
    pub fn with_recipe_id(mut self, recipe_id: impl Into<String>) -> Self {
        self.recipe_id = recipe_id.into();
        self
    }

    /// Sets the Hugging Face model the trainer loads for the base.
    pub fn with_huggingface_base(mut self, base: impl Into<String>) -> Self {
        self.huggingface_base = base.into();
        self
    }

    /// Sets how many images the job's `dataset/` holds.
    pub fn with_image_count(mut self, image_count: usize) -> Self {
        self.image_count = image_count;
        self
    }

    /// Lists a secret written beside `job.json`, so it is removed once the
    /// job ends however it ends.
    pub fn with_secret(mut self, name: impl Into<String>) -> Self {
        let name = name.into();
        if !self.secrets.contains(&name) {
            self.secrets.push(name);
        }
        self
    }

    /// Adds a key of the host's own to `job.json`, such as the compute
    /// provider its trainer runs on. A key `job.json` already gives a meaning
    /// is left as it is.
    pub fn with_field(mut self, name: impl Into<String>, value: impl Into<Value>) -> Self {
        let name = name.into();
        if !is_reserved(&name) {
            self.fields.insert(name, value.into());
        }
        self
    }

    /// Whether the job is still waiting for, or holding, the host trainer.
    pub fn busy(&self) -> bool {
        self.status.busy()
    }
}

/// Whether `name` is a key `job.json` gives a meaning of its own.
pub(crate) fn is_reserved(name: &str) -> bool {
    KEYS.contains(&name)
}
