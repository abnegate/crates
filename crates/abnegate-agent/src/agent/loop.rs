use abnegate_llm::{LlmClient, Message, RequestOptions, Role, ToolCall};
use futures::future::join_all;
use std::any::Any;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;
use tokio::time::timeout;
use uuid::Uuid;

use super::{
    AgentCallback, AgentConfig, AgentError, AgentPhase, AgentState, AgentStep, ToolCallResult,
};
use crate::context::{self, ContextSource, Entry, Policy};
use crate::tools::{ToolContext, ToolError, ToolRegistry, ToolResult};

/// Prefix of a tool call id minted for a call the provider left unnamed or
/// named the same as an earlier one.
const MINTED_CALL_PREFIX: &str = "call_";

/// A ReAct loop: think, call tools, observe their results, until the model
/// answers or the iteration budget runs out.
pub struct Agent {
    llm: LlmClient,
    tools: ToolRegistry,
    config: AgentConfig,
    context: Arc<ToolContext>,
    policy: Policy,
    guidance: Option<String>,
}

impl Agent {
    pub fn new(
        llm: LlmClient,
        tools: ToolRegistry,
        config: AgentConfig,
        context: ToolContext,
    ) -> Self {
        Self {
            llm,
            tools,
            policy: Policy {
                limit: None,
                reserved: config.max_tokens,
                source: ContextSource::Unknown,
            },
            config,
            context: Arc::new(context),
            guidance: None,
        }
    }

    /// Use a verified/configured effective capacity without guessing from the model name.
    pub fn with_context_policy(mut self, policy: Policy) -> Self {
        self.policy = policy;
        self
    }

    /// Text appended to the default system prompt, such as what an MCP
    /// server's tools are for. Ignored when the config names its own prompt.
    pub fn with_guidance(mut self, guidance: impl Into<Option<String>>) -> Self {
        self.guidance = guidance.into();
        self
    }

    /// Run the agent on a new prompt.
    pub async fn run(
        &self,
        prompt: impl Into<String>,
        callback: &dyn AgentCallback,
    ) -> Result<AgentState, AgentError> {
        let system_prompt = self
            .config
            .system_prompt
            .clone()
            .unwrap_or_else(|| default_system_prompt(&self.tools, self.guidance.as_deref()));

        let mut state = AgentState::new(prompt, Some(system_prompt));

        self.run_loop(&mut state, callback).await?;

        Ok(state)
    }

    /// Continue a finished or failed run with another user message.
    pub async fn continue_run(
        &self,
        state: &mut AgentState,
        user_message: impl Into<String>,
        callback: &dyn AgentCallback,
    ) -> Result<(), AgentError> {
        state.add_message(Message::user(user_message));
        state.phase = AgentPhase::Thinking;
        state.iteration = 0;
        state.finished = false;
        state.final_response = None;
        state.error = None;

        self.run_loop(state, callback).await
    }

    async fn run_loop(
        &self,
        state: &mut AgentState,
        callback: &dyn AgentCallback,
    ) -> Result<(), AgentError> {
        let tool_definitions = self.tools.definitions();

        loop {
            if state.iteration >= self.config.max_iterations {
                state.fail("Maximum iterations exceeded");
                return Err(AgentError::MaxIterations);
            }

            state.iteration += 1;

            state.phase = AgentPhase::Thinking;
            callback.on_phase_change(AgentPhase::Thinking, None);

            let mut step = AgentStep::new(AgentPhase::Thinking);

            let latest = state
                .messages
                .iter()
                .rposition(|message| message.role == Role::User);
            let entries: Vec<_> = state
                .messages
                .iter()
                .enumerate()
                .map(|(index, message)| Entry {
                    id: format!("{}:{index}", state.id),
                    message: message.clone(),
                    preserve: message.role == Role::System || Some(index) == latest,
                    consumed: index < state.consumed,
                })
                .collect();
            let prepared = context::prepare(
                &self.llm,
                &self.llm.config().default_model,
                &entries,
                Some(&tool_definitions),
                &self.policy,
                state.summary.as_ref(),
            )
            .await?;
            let response = self
                .llm
                .chat_with_options(
                    &self.llm.config().default_model,
                    &prepared.messages,
                    Some(&tool_definitions),
                    RequestOptions {
                        reserved: self.policy.reserved,
                    },
                )
                .await?;
            state.summary = prepared.summary;
            state.consumed = state.messages.len();

            if let Some(usage) = &response.usage {
                state.tokens_used = state.tokens_used.saturating_add(usage.total_tokens);
            }

            let choice = response
                .choices
                .first()
                .ok_or_else(|| AgentError::Tool("No response from LLM".to_string()))?;

            let mut message = choice.message.clone();
            let mut identifiers: HashSet<_> = state
                .messages
                .iter()
                .filter_map(|message| message.tool_calls.as_ref())
                .flatten()
                .map(|call| call.id.clone())
                .collect();
            if let Some(calls) = message.tool_calls.as_mut() {
                for call in calls {
                    if call.id.is_empty() || !identifiers.insert(call.id.clone()) {
                        loop {
                            let id = format!("{MINTED_CALL_PREFIX}{}", Uuid::new_v4().simple());
                            if identifiers.insert(id.clone()) {
                                call.id = id;
                                break;
                            }
                        }
                    }
                }
            }

            if let Some(tool_calls) = &message.tool_calls
                && !tool_calls.is_empty()
            {
                state.phase = AgentPhase::Acting;
                callback.on_phase_change(AgentPhase::Acting, None);

                state.add_message(message.clone());

                let (tool_results, ended) = self.act(state, callback, tool_calls).await;
                step.tool_calls = Some(tool_results);
                step = step.complete();
                state.add_step(step);

                if let Some(response) = ended {
                    Self::respond(state, callback, &response);
                    return Ok(());
                }

                state.phase = AgentPhase::Observing;
                callback.on_phase_change(AgentPhase::Observing, None);

                continue;
            }

            if let Some(content) = &message.content {
                state.add_message(message.clone());
                step.message = Some(message.clone());
                step = step.complete();
                state.add_step(step);

                if choice.finish_reason.as_deref() == Some("stop") {
                    Self::respond(state, callback, content);
                    return Ok(());
                }
            }

            if message.content.is_none() && message.tool_calls.is_none() {
                state.fail("LLM returned empty response");
                return Err(AgentError::Tool("Empty response from LLM".to_string()));
            }
        }
    }

    fn respond(state: &mut AgentState, callback: &dyn AgentCallback, response: &str) {
        state.phase = AgentPhase::Responding;
        callback.on_phase_change(AgentPhase::Responding, Some(response));
        callback.on_response(response);
        state.complete(response);
    }

    /// Run the calls one model round asked for, and say whether one of them
    /// ended the turn, with what it returned.
    ///
    /// Consecutive calls that change nothing run together; anything that
    /// mutates, or could end the turn, runs alone and in order, so a later
    /// read sees an earlier write and nothing runs after the turn has ended.
    /// A call left behind by the end of the turn is answered as not run, so
    /// every call the model made still has a result.
    async fn act(
        &self,
        state: &mut AgentState,
        callback: &dyn AgentCallback,
        calls: &[ToolCall],
    ) -> (Vec<ToolCallResult>, Option<String>) {
        let mut results = Vec::with_capacity(calls.len());
        let mut rest = calls;
        while let Some(first) = rest.first() {
            let batch = rest
                .iter()
                .take_while(|call| self.batchable(&call.function.name))
                .count();
            if batch > 1 {
                let (parallel, tail) = rest.split_at(batch);
                for call in parallel {
                    callback.on_tool_call(&call.function.name, &call.function.arguments);
                }
                let executed = join_all(parallel.iter().map(|call| async move {
                    let start = Instant::now();
                    let result = self.execute_tool(call, callback).await;
                    (call, result, elapsed(start))
                }))
                .await;
                for (call, result, duration) in executed {
                    self.record_tool(state, callback, &mut results, call, result, duration);
                }
                rest = tail;
                continue;
            }

            callback.on_tool_call(&first.function.name, &first.function.arguments);
            let start = Instant::now();
            let result = self.execute_tool(first, callback).await;
            let ends = result.success && self.tools.ends_turn(&first.function.name) == Some(true);
            let response = result.to_message();
            self.record_tool(state, callback, &mut results, first, result, elapsed(start));
            rest = &rest[1..];

            if ends {
                for skipped in rest {
                    let result = ToolResult::error(format!(
                        "Not run: {} ended the turn first.",
                        first.function.name
                    ));
                    let output = result.to_message();
                    results.push(ToolCallResult {
                        call: skipped.clone(),
                        result: output.clone(),
                        success: false,
                        duration_milliseconds: 0,
                    });
                    state.add_message(Message::tool_result(&skipped.id, output));
                }
                return (results, Some(response));
            }
        }
        (results, None)
    }

    /// Whether a call may share a batch: it changes nothing and cannot end
    /// the turn out from under the calls beside it.
    fn batchable(&self, name: &str) -> bool {
        !self.tools.mutating(name) && self.tools.ends_turn(name) != Some(true)
    }

    fn record_tool(
        &self,
        state: &mut AgentState,
        callback: &dyn AgentCallback,
        tool_results: &mut Vec<ToolCallResult>,
        tool_call: &ToolCall,
        result: ToolResult,
        duration_milliseconds: u64,
    ) {
        callback.on_tool_result(&tool_call.function.name, &result);
        let output = result.to_message();
        tool_results.push(ToolCallResult {
            call: tool_call.clone(),
            result: output.clone(),
            success: result.success,
            duration_milliseconds,
        });
        state.add_message(Message::tool_result(&tool_call.id, output));
    }

    /// Run one call on a task of its own, once the callback has approved it,
    /// for no longer than the tool's own timeout.
    ///
    /// A tool that panics or overruns fails its own call rather than the run.
    async fn execute_tool(&self, tool_call: &ToolCall, callback: &dyn AgentCallback) -> ToolResult {
        let name = &tool_call.function.name;
        let Some(tool) = self.tools.get(name) else {
            return ToolResult::error(ToolError::NotFound(name.clone()).to_string());
        };
        let parameters: serde_json::Value =
            match serde_json::from_str(&tool_call.function.arguments) {
                Ok(parameters) => parameters,
                Err(error) => {
                    return ToolResult::error(format!("Invalid tool arguments: {error}"));
                }
            };
        if !callback.approve(tool_call, tool.tier()) {
            return ToolResult::error(format!("{name} was not run: the call was not approved."));
        }

        let limit = tool.timeout(&self.context);
        let context = Arc::clone(&self.context);
        let task = tokio::spawn(async move { tool.execute(parameters, &context).await });
        let abort = task.abort_handle();
        match timeout(limit, task).await {
            Ok(Ok(Ok(result))) => result,
            Ok(Ok(Err(error))) => ToolResult::error(error.to_string()),
            Ok(Err(error)) if error.is_panic() => ToolResult::error(format!(
                "Tool {name} failed unexpectedly: {}",
                panic_message(error.into_panic())
            )),
            Ok(Err(_)) => {
                ToolResult::error(format!("Tool {name} was cancelled before it finished"))
            }
            Err(_) => {
                abort.abort();
                ToolResult::error(format!(
                    "Tool {name} timed out after {} seconds and was stopped",
                    limit.as_secs()
                ))
            }
        }
    }
}

fn elapsed(start: Instant) -> u64 {
    u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// The text a panic carried, when it carried any.
fn panic_message(payload: Box<dyn Any + Send>) -> String {
    payload
        .downcast::<String>()
        .map(|message| *message)
        .or_else(|payload| {
            payload
                .downcast::<&str>()
                .map(|message| message.to_string())
        })
        .unwrap_or_else(|_| "no message".to_string())
}

fn default_system_prompt(tools: &ToolRegistry, guidance: Option<&str>) -> String {
    let mut names: Vec<&str> = tools.names();
    names.sort_unstable();
    let list = names
        .iter()
        .map(|name| format!("- {name}"))
        .collect::<Vec<_>>()
        .join("\n");

    let mut prompt = format!(
        "You are a helpful AI assistant that can read and write code, run commands, and help with software development tasks.\n\n\
You have access to the following tools:\n\
{list}\n\n\
When working on tasks:\n\
1. First understand the current state by reading relevant files\n\
2. Plan your approach before making changes\n\
3. Make changes incrementally and verify each step\n\
4. If a command fails, analyze the error and try a different approach\n\
5. Provide clear explanations of what you're doing\n\n\
Always be careful when modifying files and running commands. If you're unsure about something, explain your uncertainty and ask for clarification."
    );

    if let Some(guidance) = guidance {
        prompt.push_str("\n\n");
        prompt.push_str(guidance);
    }

    prompt
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_prompt_lists_every_tool_in_order() {
        let registry = ToolRegistry::with_defaults();
        let prompt = default_system_prompt(&registry, None);
        let listed: Vec<&str> = prompt
            .lines()
            .filter_map(|line| line.strip_prefix("- "))
            .collect();
        let mut names = registry.names();
        names.sort_unstable();
        assert_eq!(listed, names);
    }

    #[test]
    fn guidance_is_appended_to_the_default_prompt() {
        let prompt = default_system_prompt(&ToolRegistry::new(), Some("Prefer docs_search."));
        assert!(prompt.ends_with("\n\nPrefer docs_search."), "{prompt}");
    }

    #[test]
    fn the_default_prompt_carries_no_guidance_unless_given_some() {
        let prompt = default_system_prompt(&ToolRegistry::new(), None);
        assert!(prompt.ends_with("ask for clarification."), "{prompt}");
    }
}
