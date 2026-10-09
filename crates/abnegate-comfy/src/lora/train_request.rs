use crate::host::Compute;
use crate::lora::TrainImage;
use crate::lora::TrainMethod;
use crate::lora::TrainSubject;
use abnegate_secret::SecretValue;
use serde::Deserialize;
use serde_json::Map;
use serde_json::Value;
use std::collections::BTreeMap;

/// A weight to train: its name, the base recipe it is trained on, what the
/// images show, how they are trained, and the images.
#[derive(Debug, Deserialize)]
#[non_exhaustive]
pub struct TrainRequest {
    /// Filename the weight is published under, with or without `.safetensors`.
    pub name: String,
    /// Recipe id of the base the weight is trained on.
    pub base: String,
    /// Word an identity weight is prompted with. Edit bases need none.
    #[serde(default)]
    pub trigger: Option<String>,
    /// What the images show.
    #[serde(default)]
    pub subject: TrainSubject,
    /// How the images become a weight.
    #[serde(default)]
    pub method: TrainMethod,
    /// The training set. A request that names none reads, so its images can
    /// follow in batches too large for one body, but trains only once they
    /// are there.
    #[serde(default)]
    pub images: Vec<TrainImage>,
    /// Where the host trainer runs the request, under the `provider` key. A
    /// [remote](Compute::is_remote) compute trains every subject and method
    /// on the [host trainer](crate::Config::host_trainer) and needs its
    /// [`compute_key`](TrainRequest::compute_key).
    #[serde(default, rename = "provider")]
    pub compute: Compute,
    /// The credential of a compute with a [key file](Compute::key_file),
    /// written there beside the host job, readable only by its owner, and
    /// never into `job.json`.
    #[serde(skip)]
    pub compute_key: Option<SecretValue>,
    /// Keys of the host's own added to the
    /// [`host::Job`](crate::host::Job) when the request trains on the
    /// [host trainer](crate::Config::host_trainer), such as a setting only
    /// its trainer reads. Ignored otherwise.
    #[serde(skip)]
    pub host_fields: Map<String, Value>,
    /// Credentials written beside the host job, one file each, readable only
    /// by their owner, when the request trains on the host trainer. Ignored
    /// otherwise.
    #[serde(skip)]
    pub host_secrets: BTreeMap<String, SecretValue>,
}

impl TrainRequest {
    /// Trains a LoRA named `name` on the base recipe `base` from `images`,
    /// with no trigger word, as [`TrainSubject::Other`].
    pub fn new(name: impl Into<String>, base: impl Into<String>, images: Vec<TrainImage>) -> Self {
        Self {
            name: name.into(),
            base: base.into(),
            trigger: None,
            subject: TrainSubject::default(),
            method: TrainMethod::default(),
            images,
            compute: Compute::Local,
            compute_key: None,
            host_fields: Map::new(),
            host_secrets: BTreeMap::new(),
        }
    }

    /// Sets the word an identity weight is prompted with.
    pub fn with_trigger(mut self, trigger: impl Into<String>) -> Self {
        self.trigger = Some(trigger.into());
        self
    }

    /// Sets what the images show.
    pub fn with_subject(mut self, subject: TrainSubject) -> Self {
        self.subject = subject;
        self
    }

    /// Sets how the images become a weight.
    pub fn with_method(mut self, method: TrainMethod) -> Self {
        self.method = method;
        self
    }

    /// Sets where the host trainer runs the request.
    pub fn with_compute(mut self, compute: Compute) -> Self {
        self.compute = compute;
        self
    }

    /// Sets the credential the compute's trainer reads from its
    /// [key file](Compute::key_file).
    pub fn with_compute_key(mut self, key: SecretValue) -> Self {
        self.compute_key = Some(key);
        self
    }

    /// Adds a key of the host's own to the job the host trainer reads.
    pub fn with_host_field(mut self, name: impl Into<String>, value: impl Into<Value>) -> Self {
        self.host_fields.insert(name.into(), value.into());
        self
    }

    /// Adds a credential the host trainer reads from the file `name` beside
    /// its job.
    pub fn with_host_secret(mut self, name: impl Into<String>, value: SecretValue) -> Self {
        self.host_secrets.insert(name.into(), value);
        self
    }
}
