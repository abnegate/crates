use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use async_trait::async_trait;

use crate::error::Error;
use crate::provider::capabilities::Capabilities;
use crate::provider::completion::Completion;
use crate::provider::completion_provider::CompletionProvider;
use crate::provider::error::ProviderError;
use crate::provider::kind::ProviderKind;
use crate::provider::request::CompletionRequest;
use crate::provider::testing::behaviour::Behaviour;
use crate::provider::testing::seen::Seen;
use crate::wire::Message;
use crate::wire::ToolDefinition;
use crate::wire::Usage;

/// A provider whose answers a test decides, and which remembers how often it
/// was asked and what it was last handed.
#[derive(Debug)]
pub struct StubProvider {
    name: String,
    behaviour: Behaviour,
    capabilities: Capabilities,
    kind: ProviderKind,
    usage: Option<Usage>,
    calls: AtomicUsize,
    seen: Mutex<Option<Seen>>,
}

impl StubProvider {
    /// A stub called `name` that does as `behaviour` says, reporting itself
    /// as an HTTP provider with no capabilities.
    pub fn new(name: impl Into<String>, behaviour: Behaviour) -> Self {
        Self {
            name: name.into(),
            behaviour,
            capabilities: Capabilities::NONE,
            kind: ProviderKind::Http,
            usage: None,
            calls: AtomicUsize::new(0),
            seen: Mutex::new(None),
        }
    }

    /// A stub that answers every request with `text`.
    pub fn answering(name: impl Into<String>, text: impl Into<String>) -> Self {
        Self::new(name, Behaviour::Answer(text.into()))
    }

    /// A stub that fails every request in a way a chain moves past.
    pub fn failing(name: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(name, Behaviour::Fail(message.into()))
    }

    /// A stub that fails every request in a way a chain must not move past.
    pub fn rejecting(name: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(name, Behaviour::Reject(message.into()))
    }

    /// Report `capabilities`.
    pub fn with_capabilities(mut self, capabilities: Capabilities) -> Self {
        self.capabilities = capabilities;
        self
    }

    /// Report itself as `kind`.
    pub fn with_kind(mut self, kind: ProviderKind) -> Self {
        self.kind = kind;
        self
    }

    /// Report `usage` with every answer.
    pub fn with_usage(mut self, usage: Usage) -> Self {
        self.usage = Some(usage);
        self
    }

    /// The stub behind a shared handle, as a router takes it.
    pub fn shared(self) -> Arc<dyn CompletionProvider> {
        Arc::new(self)
    }

    /// How many requests the stub has been handed.
    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    /// The last request the stub was handed, or `None` before the first.
    pub fn seen(&self) -> Option<Seen> {
        self.seen
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

#[async_trait]
impl CompletionProvider for StubProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn kind(&self) -> ProviderKind {
        self.kind
    }

    fn capabilities(&self) -> Capabilities {
        self.capabilities
    }

    async fn complete(&self, request: CompletionRequest<'_>) -> Result<Completion, ProviderError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        *self
            .seen
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Seen {
            model: request.model.to_string(),
            messages: request.messages.to_vec(),
            tools: request.tools.map(<[ToolDefinition]>::to_vec),
            options: request.options,
            response_format: request.response_format.cloned(),
            temperature: request.temperature,
        });

        match &self.behaviour {
            Behaviour::Answer(text) => Ok(Completion::new(
                self.name.clone(),
                Message::assistant(text.clone()),
            )
            .with_usage(self.usage)
            .with_finish_reason("stop".to_string())),
            Behaviour::Fail(message) => Err(ProviderError::Agent {
                provider: self.name.clone(),
                message: message.clone(),
            }),
            Behaviour::Reject(message) => Err(ProviderError::Http {
                provider: self.name.clone(),
                source: Error::Api {
                    status: 400,
                    message: message.clone(),
                },
            }),
        }
    }
}
